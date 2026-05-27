use keyring_core::Entry;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use thiserror::Error;
use uuid::Uuid;

const CREDENTIAL_SERVICE: &str = "MountMate";
const PROFILES_FILE: &str = "profiles.json";

type AppResult<T> = Result<T, MountMateError>;
type CommandResult<T> = Result<T, String>;

#[derive(Debug, Error)]
enum MountMateError {
    #[error("{0}")]
    Validation(String),
    #[error("配置文件读取失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("JSON 解析失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("系统安全存储访问失败：{0}")]
    Credential(String),
    #[error("系统命令执行失败：{0}")]
    Process(String),
    #[error("当前平台暂不支持：{0}")]
    #[allow(dead_code)]
    Unsupported(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Profile {
    id: String,
    display_name: String,
    server: String,
    share: String,
    domain: String,
    username: String,
    default_drive_letter: String,
    mount_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportConfig {
    schema_version: u8,
    display_name: String,
    server: String,
    share: String,
    #[serde(default)]
    domain: String,
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default = "default_drive_letter")]
    default_drive_letter: String,
    #[serde(default)]
    mount_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileView {
    id: String,
    display_name: String,
    server: String,
    share: String,
    domain: String,
    username: String,
    default_drive_letter: String,
    mount_name: String,
    smb_url: String,
    unc_path: String,
    mount_point: String,
    is_mounted: bool,
    has_password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ProfileStore {
    profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticCheck {
    name: String,
    status: CheckStatus,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticResult {
    profile: ProfileView,
    checks: Vec<DiagnosticCheck>,
    suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum CheckStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkResult {
    profile: ProfileView,
    mount_point: String,
    large_file_bytes: u64,
    large_write_seconds: f64,
    large_write_mbps: f64,
    large_read_seconds: f64,
    large_read_mbps: f64,
    small_file_count: usize,
    small_file_bytes_each: usize,
    small_write_seconds: f64,
    small_files_per_second: f64,
}

#[derive(Debug, Clone)]
struct CommandSpec {
    program: String,
    args: Vec<String>,
}

fn default_drive_letter() -> String {
    "Z".to_string()
}

fn to_command_result<T>(result: AppResult<T>) -> CommandResult<T> {
    result.map_err(|error| error.to_string())
}

#[tauri::command]
async fn import_config(app: AppHandle, file: String) -> CommandResult<ProfileView> {
    run_blocking(move || import_config_inner(&app, &file)).await
}

#[tauri::command]
async fn list_profiles(app: AppHandle) -> CommandResult<Vec<ProfileView>> {
    run_blocking(move || load_store(&app).and_then(|store| profiles_to_views(store.profiles))).await
}

#[tauri::command]
async fn mount_profile(app: AppHandle, profile_id: String) -> CommandResult<ProfileView> {
    run_blocking(move || with_profile(&app, &profile_id, |profile| {
        let password = get_password(profile)?;
        platform::mount(profile, &password)?;
        profile_to_view(profile)
    }))
    .await
}

#[tauri::command]
async fn unmount_profile(app: AppHandle, profile_id: String) -> CommandResult<ProfileView> {
    run_blocking(move || with_profile(&app, &profile_id, |profile| {
        platform::unmount(profile)?;
        profile_to_view(profile)
    }))
    .await
}

#[tauri::command]
async fn open_mount(app: AppHandle, profile_id: String) -> CommandResult<()> {
    run_blocking(move || with_profile(&app, &profile_id, |profile| platform::open_mount(profile)))
        .await
}

#[tauri::command]
async fn test_connection(app: AppHandle, profile_id: String) -> CommandResult<DiagnosticResult> {
    run_blocking(move || with_profile(&app, &profile_id, |profile| {
        let mut checks = Vec::new();
        let mut suggestions = Vec::new();

        match check_smb_port(&profile.server) {
            Ok(ms) => checks.push(DiagnosticCheck {
                name: "SMB 端口".to_string(),
                status: CheckStatus::Ok,
                message: format!("{}:445 可连接，耗时 {} ms", profile.server, ms),
            }),
            Err(message) => {
                checks.push(DiagnosticCheck {
                    name: "SMB 端口".to_string(),
                    status: CheckStatus::Error,
                    message,
                });
                suggestions
                    .push("确认电脑与服务器在同一网络，服务器防火墙允许 TCP 445。".to_string());
            }
        }

        match get_password(profile) {
            Ok(password) => {
                checks.push(DiagnosticCheck {
                    name: "凭据".to_string(),
                    status: CheckStatus::Ok,
                    message: "密码已保存在系统安全存储。".to_string(),
                });
                checks.push(platform::probe_share(profile, &password));
            }
            Err(error) => {
                checks.push(DiagnosticCheck {
                    name: "凭据".to_string(),
                    status: CheckStatus::Error,
                    message: error.to_string(),
                });
                suggestions.push(
                    "重新导入包含 password 字段的配置包，导入后本地配置不会保存明文密码。"
                        .to_string(),
                );
            }
        }

        if platform::is_mounted(profile) {
            checks.push(DiagnosticCheck {
                name: "挂载状态".to_string(),
                status: CheckStatus::Ok,
                message: format!("已挂载：{}", platform::mount_point(profile)),
            });
        } else {
            checks.push(DiagnosticCheck {
                name: "挂载状态".to_string(),
                status: CheckStatus::Warning,
                message: "当前未挂载。".to_string(),
            });
            suggestions.push(
                "先点击挂载；如果失败，错误信息通常能区分密码错误、共享名错误或权限不足。"
                    .to_string(),
            );
        }

        Ok(DiagnosticResult {
            profile: profile_to_view(profile)?,
            checks,
            suggestions,
        })
    }))
    .await
}

#[tauri::command]
async fn benchmark(app: AppHandle, profile_id: String) -> CommandResult<BenchmarkResult> {
    run_blocking(move || with_profile(&app, &profile_id, |profile| benchmark_inner(profile))).await
}

async fn run_blocking<T>(
    task: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> CommandResult<T>
where
    T: Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(task).await {
        Ok(result) => to_command_result(result),
        Err(error) => Err(format!("后台任务执行失败：{error}")),
    }
}

fn import_config_inner(app: &AppHandle, source: &str) -> AppResult<ProfileView> {
    let raw = read_import_source(source)?;
    let config: ImportConfig = serde_json::from_str(&raw)?;
    let (profile, password) = config.into_profile()?;

    if password.trim().is_empty() {
        return Err(MountMateError::Validation(
            "配置包缺少 password，无法完成一键挂载。".to_string(),
        ));
    }

    let mut store = load_store(app)?;
    let profile = upsert_profile(&mut store, profile);
    save_password(&profile, &password)?;
    save_store(app, &store)?;
    profile_to_view(&profile)
}

impl ImportConfig {
    fn into_profile(self) -> AppResult<(Profile, String)> {
        if self.schema_version != 1 {
            return Err(MountMateError::Validation(
                "schemaVersion 目前只支持 1。".to_string(),
            ));
        }

        validate_required("displayName", &self.display_name)?;
        validate_required("server", &self.server)?;
        validate_required("share", &self.share)?;
        validate_required("username", &self.username)?;

        let drive = normalize_drive_letter(&self.default_drive_letter)?;
        let mount_name = if self.mount_name.trim().is_empty() {
            self.share.trim().to_string()
        } else {
            sanitize_mount_name(&self.mount_name)?
        };

        Ok((
            Profile {
                id: Uuid::new_v4().to_string(),
                display_name: self.display_name.trim().to_string(),
                server: self.server.trim().to_string(),
                share: self.share.trim().trim_matches('/').to_string(),
                domain: self.domain.trim().to_string(),
                username: self.username.trim().to_string(),
                default_drive_letter: drive,
                mount_name,
            },
            self.password,
        ))
    }
}

fn read_import_source(source: &str) -> AppResult<String> {
    if source.trim_start().starts_with('{') {
        Ok(source.to_string())
    } else {
        Ok(fs::read_to_string(source)?)
    }
}

fn load_store(app: &AppHandle) -> AppResult<ProfileStore> {
    let path = profiles_path(app)?;
    if !path.exists() {
        return Ok(ProfileStore::default());
    }

    let contents = fs::read_to_string(path)?;
    if contents.trim().is_empty() {
        return Ok(ProfileStore::default());
    }

    Ok(serde_json::from_str(&contents)?)
}

fn save_store(app: &AppHandle, store: &ProfileStore) -> AppResult<()> {
    let path = profiles_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = path.with_extension("json.tmp");
    let contents = serde_json::to_string_pretty(store)?;
    fs::write(&temp_path, contents)?;
    fs::rename(temp_path, path)?;
    Ok(())
}

fn profiles_path(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app.path().app_config_dir().map_err(|error| {
        MountMateError::Io(std::io::Error::new(std::io::ErrorKind::Other, error))
    })?;
    fs::create_dir_all(&dir)?;
    Ok(dir.join(PROFILES_FILE))
}

fn upsert_profile(store: &mut ProfileStore, profile: Profile) -> Profile {
    if let Some(existing) = store.profiles.iter_mut().find(|item| {
        item.server.eq_ignore_ascii_case(&profile.server)
            && item.share.eq_ignore_ascii_case(&profile.share)
            && item.username.eq_ignore_ascii_case(&profile.username)
            && item.domain.eq_ignore_ascii_case(&profile.domain)
    }) {
        let id = existing.id.clone();
        *existing = Profile { id, ..profile };
        existing.clone()
    } else {
        store.profiles.push(profile.clone());
        profile
    }
}

fn with_profile<T>(
    app: &AppHandle,
    profile_id: &str,
    action: impl FnOnce(&Profile) -> AppResult<T>,
) -> AppResult<T> {
    let store = load_store(app)?;
    let profile = store
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| MountMateError::Validation("未找到该共享盘配置。".to_string()))?;
    action(profile)
}

fn profiles_to_views(profiles: Vec<Profile>) -> AppResult<Vec<ProfileView>> {
    profiles
        .iter()
        .map(profile_to_view)
        .collect::<AppResult<Vec<_>>>()
}

fn profile_to_view(profile: &Profile) -> AppResult<ProfileView> {
    Ok(ProfileView {
        id: profile.id.clone(),
        display_name: profile.display_name.clone(),
        server: profile.server.clone(),
        share: profile.share.clone(),
        domain: profile.domain.clone(),
        username: profile.username.clone(),
        default_drive_letter: profile.default_drive_letter.clone(),
        mount_name: profile.mount_name.clone(),
        smb_url: mac_smb_url(profile),
        unc_path: windows_unc_path(profile),
        mount_point: platform::mount_point(profile),
        is_mounted: platform::is_mounted(profile),
        has_password: true,
    })
}

fn validate_required(field: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty() {
        return Err(MountMateError::Validation(format!("{field} 不能为空。")));
    }
    Ok(())
}

fn normalize_drive_letter(value: &str) -> AppResult<String> {
    let value = value.trim().trim_end_matches(':');
    let mut chars = value.chars();
    let letter = chars.next().ok_or_else(|| {
        MountMateError::Validation("defaultDriveLetter 不能为空，例如 Z。".to_string())
    })?;

    if chars.next().is_some() || !letter.is_ascii_alphabetic() {
        return Err(MountMateError::Validation(
            "defaultDriveLetter 必须是单个英文字母，例如 Z。".to_string(),
        ));
    }

    Ok(letter.to_ascii_uppercase().to_string())
}

fn sanitize_mount_name(value: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() || value.contains('/') || value.contains(':') || value.contains('\0') {
        return Err(MountMateError::Validation(
            "mountName 不能包含 /、: 或空字符。".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn credential_key(profile: &Profile) -> String {
    format!(
        "{}|{}|{}|{}",
        profile.server, profile.share, profile.domain, profile.username
    )
}

fn credential_entry(profile: &Profile) -> AppResult<Entry> {
    ensure_keyring()?;
    Entry::new(CREDENTIAL_SERVICE, &credential_key(profile))
        .map_err(|error| MountMateError::Credential(error.to_string()))
}

fn ensure_keyring() -> AppResult<()> {
    static KEYRING_INIT: OnceLock<Result<(), String>> = OnceLock::new();
    KEYRING_INIT
        .get_or_init(|| keyring::use_native_store(false).map_err(|error| error.to_string()))
        .clone()
        .map_err(MountMateError::Credential)
}

fn save_password(profile: &Profile, password: &str) -> AppResult<()> {
    credential_entry(profile)?
        .set_password(password)
        .map_err(|error| MountMateError::Credential(error.to_string()))
}

fn get_password(profile: &Profile) -> AppResult<String> {
    credential_entry(profile)?
        .get_password()
        .map_err(|error| MountMateError::Credential(error.to_string()))
}

fn check_smb_port(server: &str) -> Result<u128, String> {
    let start = Instant::now();
    let timeout = Duration::from_millis(2500);
    let addresses = (server, 445)
        .to_socket_addrs()
        .map_err(|error| format!("无法解析服务器地址 {server}: {error}"))?;

    for address in addresses {
        if TcpStream::connect_timeout(&address, timeout).is_ok() {
            return Ok(start.elapsed().as_millis());
        }
    }

    Err(format!("{server}:445 不可达或超时。"))
}

fn benchmark_inner(profile: &Profile) -> AppResult<BenchmarkResult> {
    if !platform::is_mounted(profile) {
        return Err(MountMateError::Validation(
            "共享盘尚未挂载，无法测速。".to_string(),
        ));
    }

    let mount_dir = platform::mounted_path(profile)
        .ok_or_else(|| MountMateError::Validation("找不到已挂载目录，无法测速。".to_string()))?;

    let bench_dir = mount_dir.join(format!(".mountmate-benchmark-{}", Uuid::new_v4()));
    fs::create_dir_all(&bench_dir)?;

    let result = run_benchmark_files(profile, &mount_dir, &bench_dir);
    let _ = fs::remove_dir_all(&bench_dir);
    result
}

fn run_benchmark_files(
    profile: &Profile,
    mount_dir: &Path,
    bench_dir: &Path,
) -> AppResult<BenchmarkResult> {
    let large_size = 8 * 1024 * 1024;
    let large_path = bench_dir.join("large.bin");
    let large_data = vec![0x5a; large_size];

    let started = Instant::now();
    fs::write(&large_path, &large_data)?;
    let large_write_seconds = elapsed_seconds(started);

    let started = Instant::now();
    let read_data = fs::read(&large_path)?;
    let large_read_seconds = elapsed_seconds(started);
    if read_data.len() != large_size {
        return Err(MountMateError::Process(
            "大文件读取长度不一致。".to_string(),
        ));
    }

    let small_file_count = 20;
    let small_file_bytes_each = 4096;
    let small_data = vec![0x33; small_file_bytes_each];
    let started = Instant::now();
    for index in 0..small_file_count {
        fs::write(bench_dir.join(format!("small-{index:02}.bin")), &small_data)?;
    }
    let small_write_seconds = elapsed_seconds(started);

    let large_write_mbps = mbps(large_size as u64, large_write_seconds);
    let large_read_mbps = mbps(large_size as u64, large_read_seconds);
    let small_files_per_second = small_file_count as f64 / small_write_seconds.max(0.001);

    Ok(BenchmarkResult {
        profile: profile_to_view(profile)?,
        mount_point: mount_dir.to_string_lossy().to_string(),
        large_file_bytes: large_size as u64,
        large_write_seconds,
        large_write_mbps,
        large_read_seconds,
        large_read_mbps,
        small_file_count,
        small_file_bytes_each,
        small_write_seconds,
        small_files_per_second,
    })
}

fn elapsed_seconds(started: Instant) -> f64 {
    started.elapsed().as_secs_f64().max(0.001)
}

fn mbps(bytes: u64, seconds: f64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0 / seconds.max(0.001)
}

fn run_command(
    spec: CommandSpec,
    stdin_data: Option<&str>,
    secret_for_redaction: Option<&str>,
) -> AppResult<String> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin_data.is_some() {
        command.stdin(Stdio::piped());
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let mut child = command
        .spawn()
        .map_err(|error| MountMateError::Process(format!("无法启动 {}：{error}", spec.program)))?;

    if let Some(input) = stdin_data {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| MountMateError::Process("无法写入命令 stdin。".to_string()))?;
        stdin.write_all(input.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        return Ok(stdout);
    }

    let mut message = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };

    if let Some(secret) = secret_for_redaction {
        message = redact_secret(&message, secret);
    }

    Err(MountMateError::Process(classify_mount_error(&message)))
}

fn redact_secret(value: &str, secret: &str) -> String {
    if secret.is_empty() {
        value.to_string()
    } else {
        value.replace(secret, "[redacted]")
    }
}

fn classify_mount_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("password")
        || lower.contains("denied")
        || lower.contains("auth")
        || lower.contains("logon failure")
        || message.contains("密码")
        || message.contains("认证")
        || message.contains("拒绝")
    {
        format!(
            "认证失败，请检查用户名、密码或 SMB 权限。原始错误：{}",
            message.trim()
        )
    } else if lower.contains("not found")
        || lower.contains("bad network name")
        || lower.contains("cannot find")
        || lower.contains("cannot be found")
        || message.contains("不存在")
        || message.contains("找不到")
    {
        format!(
            "共享名可能错误，请检查 share 字段。原始错误：{}",
            message.trim()
        )
    } else {
        message.trim().to_string()
    }
}

fn qualified_username(profile: &Profile) -> String {
    if profile.domain.trim().is_empty() {
        profile.username.clone()
    } else if cfg!(target_os = "macos") {
        format!("{};{}", profile.domain, profile.username)
    } else {
        format!(r"{}\{}", profile.domain, profile.username)
    }
}

fn windows_unc_path(profile: &Profile) -> String {
    format!(r"\\{}\{}", profile.server, profile.share)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_drive_mount(profile: &Profile) -> String {
    format!("{}:", profile.default_drive_letter)
}

fn mac_smb_url(profile: &Profile) -> String {
    format!(
        "smb://{}/{}",
        profile.server,
        percent_encode_segment(&profile.share)
    )
}

fn percent_encode_segment(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (*byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect::<Vec<_>>()
        .join("")
}

fn applescript_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub fn mount_point(profile: &Profile) -> String {
        format!("/Volumes/{}", profile.mount_name)
    }

    pub fn is_mounted(profile: &Profile) -> bool {
        mounted_path(profile).is_some()
    }

    pub fn mounted_path(profile: &Profile) -> Option<PathBuf> {
        candidate_mount_paths(profile)
            .into_iter()
            .find(|path| path.exists() && mount_output_contains(path))
    }

    pub fn mount(profile: &Profile, password: &str) -> AppResult<()> {
        if is_mounted(profile) {
            return Ok(());
        }

        let script = format!(
            "mount volume \"{}\" as user name \"{}\" with password \"{}\"\n",
            applescript_escape(&mac_smb_url(profile)),
            applescript_escape(&qualified_username(profile)),
            applescript_escape(password)
        );

        run_command(
            CommandSpec {
                program: "osascript".to_string(),
                args: vec!["-".to_string()],
            },
            Some(&script),
            Some(password),
        )?;

        if is_mounted(profile) {
            Ok(())
        } else {
            Err(MountMateError::Process(
                "系统命令已返回成功，但未检测到挂载目录。".to_string(),
            ))
        }
    }

    pub fn unmount(profile: &Profile) -> AppResult<()> {
        let paths = candidate_mount_paths(profile);
        let mut attempted = false;
        for path in paths.into_iter().filter(|path| path.exists()) {
            attempted = true;
            let spec = CommandSpec {
                program: "diskutil".to_string(),
                args: vec!["unmount".to_string(), path.to_string_lossy().to_string()],
            };
            let _ = run_command(spec, None, None);
        }

        if attempted && is_mounted(profile) {
            return Err(MountMateError::Process("断开挂载失败。".to_string()));
        }
        Ok(())
    }

    pub fn open_mount(profile: &Profile) -> AppResult<()> {
        let path = mounted_path(profile).unwrap_or_else(|| PathBuf::from(mount_point(profile)));
        if !path.exists() {
            return Err(MountMateError::Validation(
                "共享盘尚未挂载，无法打开目录。".to_string(),
            ));
        }
        run_command(
            CommandSpec {
                program: "open".to_string(),
                args: vec![path.to_string_lossy().to_string()],
            },
            None,
            None,
        )?;
        Ok(())
    }

    pub fn probe_share(profile: &Profile, _password: &str) -> DiagnosticCheck {
        if is_mounted(profile) {
            DiagnosticCheck {
                name: "共享访问".to_string(),
                status: CheckStatus::Ok,
                message: "共享盘已挂载，认证与共享名可用。".to_string(),
            }
        } else {
            DiagnosticCheck {
                name: "共享访问".to_string(),
                status: CheckStatus::Warning,
                message: "macOS 会在挂载时验证认证与共享名；当前未挂载，暂不做破坏性探测。"
                    .to_string(),
            }
        }
    }

    fn candidate_mount_paths(profile: &Profile) -> Vec<PathBuf> {
        let mut paths = vec![PathBuf::from(mount_point(profile))];
        let share_path = PathBuf::from(format!("/Volumes/{}", profile.share));
        if !paths.contains(&share_path) {
            paths.push(share_path);
        }
        paths
    }

    fn mount_output_contains(path: &Path) -> bool {
        let output = Command::new("mount").output();
        match output {
            Ok(output) => String::from_utf8_lossy(&output.stdout)
                .contains(&path.to_string_lossy().to_string()),
            Err(_) => path.exists(),
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;

    pub fn mount_point(profile: &Profile) -> String {
        format!("{}\\", windows_drive_mount(profile))
    }

    pub fn is_mounted(profile: &Profile) -> bool {
        Path::new(&mount_point(profile)).exists()
    }

    pub fn mounted_path(profile: &Profile) -> Option<PathBuf> {
        let path = PathBuf::from(mount_point(profile));
        path.exists().then_some(path)
    }

    pub fn mount(profile: &Profile, password: &str) -> AppResult<()> {
        if is_mounted(profile) {
            return Ok(());
        }

        let spec = windows_mount_command(profile);
        run_command(spec, Some(&format!("{password}\n")), Some(password))?;
        Ok(())
    }

    pub fn unmount(profile: &Profile) -> AppResult<()> {
        if !is_mounted(profile) {
            return Ok(());
        }

        run_command(
            CommandSpec {
                program: "net".to_string(),
                args: vec![
                    "use".to_string(),
                    windows_drive_mount(profile),
                    "/delete".to_string(),
                    "/y".to_string(),
                ],
            },
            None,
            None,
        )?;
        Ok(())
    }

    pub fn open_mount(profile: &Profile) -> AppResult<()> {
        if !is_mounted(profile) {
            return Err(MountMateError::Validation(
                "共享盘尚未挂载，无法打开目录。".to_string(),
            ));
        }
        run_command(
            CommandSpec {
                program: "explorer.exe".to_string(),
                args: vec![mount_point(profile)],
            },
            None,
            None,
        )?;
        Ok(())
    }

    pub fn probe_share(profile: &Profile, password: &str) -> DiagnosticCheck {
        let spec = CommandSpec {
            program: "net".to_string(),
            args: vec![
                "use".to_string(),
                windows_unc_path(profile),
                format!("/user:{}", qualified_username(profile)),
                "*".to_string(),
                "/persistent:no".to_string(),
            ],
        };

        match run_command(spec, Some(&format!("{password}\n")), Some(password)) {
            Ok(_) => {
                let _ = run_command(
                    CommandSpec {
                        program: "net".to_string(),
                        args: vec![
                            "use".to_string(),
                            windows_unc_path(profile),
                            "/delete".to_string(),
                            "/y".to_string(),
                        ],
                    },
                    None,
                    None,
                );
                DiagnosticCheck {
                    name: "共享访问".to_string(),
                    status: CheckStatus::Ok,
                    message: "认证通过，共享名可访问。".to_string(),
                }
            }
            Err(error) => DiagnosticCheck {
                name: "共享访问".to_string(),
                status: CheckStatus::Error,
                message: error.to_string(),
            },
        }
    }

    fn windows_mount_command(profile: &Profile) -> CommandSpec {
        CommandSpec {
            program: "net".to_string(),
            args: vec![
                "use".to_string(),
                windows_drive_mount(profile),
                windows_unc_path(profile),
                format!("/user:{}", qualified_username(profile)),
                "*".to_string(),
                "/persistent:yes".to_string(),
            ],
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::*;

    pub fn mount_point(_profile: &Profile) -> String {
        "unsupported".to_string()
    }

    pub fn is_mounted(_profile: &Profile) -> bool {
        false
    }

    pub fn mounted_path(_profile: &Profile) -> Option<PathBuf> {
        None
    }

    pub fn mount(_profile: &Profile, _password: &str) -> AppResult<()> {
        Err(MountMateError::Unsupported(
            "v1 只支持 Windows 和 macOS。".to_string(),
        ))
    }

    pub fn unmount(_profile: &Profile) -> AppResult<()> {
        Err(MountMateError::Unsupported(
            "v1 只支持 Windows 和 macOS。".to_string(),
        ))
    }

    pub fn open_mount(_profile: &Profile) -> AppResult<()> {
        Err(MountMateError::Unsupported(
            "v1 只支持 Windows 和 macOS。".to_string(),
        ))
    }

    pub fn probe_share(_profile: &Profile, _password: &str) -> DiagnosticCheck {
        DiagnosticCheck {
            name: "共享访问".to_string(),
            status: CheckStatus::Error,
            message: "v1 只支持 Windows 和 macOS。".to_string(),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            import_config,
            list_profiles,
            mount_profile,
            unmount_profile,
            open_mount,
            test_connection,
            benchmark
        ])
        .run(tauri::generate_context!())
        .expect("error while running MountMate");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_import() -> ImportConfig {
        serde_json::from_str(
            r#"{
              "schemaVersion": 1,
              "displayName": "CompanyDisk",
              "server": "192.168.77.174",
              "share": "CompanyDisk",
              "domain": "",
              "username": "yaoyelan",
              "password": "secret-password",
              "defaultDriveLetter": "Z",
              "mountName": "CompanyDisk"
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_and_validates_import_config() {
        let (profile, password) = sample_import().into_profile().unwrap();
        assert_eq!(profile.display_name, "CompanyDisk");
        assert_eq!(profile.server, "192.168.77.174");
        assert_eq!(profile.share, "CompanyDisk");
        assert_eq!(profile.default_drive_letter, "Z");
        assert_eq!(password, "secret-password");
    }

    #[test]
    fn builds_platform_paths() {
        let (profile, _) = sample_import().into_profile().unwrap();
        assert_eq!(windows_unc_path(&profile), r"\\192.168.77.174\CompanyDisk");
        assert_eq!(mac_smb_url(&profile), "smb://192.168.77.174/CompanyDisk");
    }

    #[test]
    fn credential_key_is_stable() {
        let (profile, _) = sample_import().into_profile().unwrap();
        assert_eq!(
            credential_key(&profile),
            "192.168.77.174|CompanyDisk||yaoyelan"
        );
    }

    #[test]
    fn profile_store_does_not_serialize_password() {
        let (profile, _) = sample_import().into_profile().unwrap();
        let store = ProfileStore {
            profiles: vec![profile],
        };
        let json = serde_json::to_string(&store).unwrap();
        assert!(!json.contains("secret-password"));
        assert!(!json.contains("\"password\""));
    }

    #[test]
    fn mount_errors_are_classified() {
        let auth_error = classify_mount_error("The password is invalid");
        assert!(auth_error.contains("认证失败"));

        let share_error = classify_mount_error("The network name cannot be found");
        assert!(share_error.contains("共享名"));
    }

    #[test]
    fn redacts_secret_from_errors() {
        assert_eq!(
            redact_secret("failed with secret-password", "secret-password"),
            "failed with [redacted]"
        );
    }
}
