import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  ClipboardPaste,
  FolderOpen,
  Gauge,
  HardDrive,
  Import,
  KeyRound,
  PlugZap,
  RefreshCw,
  Server,
  ShieldCheck,
  Unplug,
  XCircle,
} from "lucide-react";
import mountmateIcon from "./assets/mountmate-icon.png";
import "./App.css";

type Profile = {
  id: string;
  displayName: string;
  server: string;
  share: string;
  domain: string;
  username: string;
  defaultDriveLetter: string;
  mountName: string;
  smbUrl: string;
  uncPath: string;
  mountPoint: string;
  isMounted: boolean;
  hasPassword: boolean;
};

type CheckStatus = "ok" | "warning" | "error";

type DiagnosticCheck = {
  name: string;
  status: CheckStatus;
  message: string;
};

type DiagnosticResult = {
  profile: Profile;
  checks: DiagnosticCheck[];
  suggestions: string[];
};

type BenchmarkResult = {
  profile: Profile;
  mountPoint: string;
  largeFileBytes: number;
  largeWriteSeconds: number;
  largeWriteMbps: number;
  largeReadSeconds: number;
  largeReadMbps: number;
  smallFileCount: number;
  smallFileBytesEach: number;
  smallWriteSeconds: number;
  smallFilesPerSecond: number;
};

type BusyAction =
  | "load"
  | "import"
  | "mount"
  | "unmount"
  | "open"
  | "diagnose"
  | "benchmark"
  | null;

function App() {
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [selectedId, setSelectedId] = useState<string>("");
  const [diagnostic, setDiagnostic] = useState<DiagnosticResult | null>(null);
  const [benchmark, setBenchmark] = useState<BenchmarkResult | null>(null);
  const [notice, setNotice] = useState("准备就绪");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<BusyAction>("load");
  const [isDragging, setIsDragging] = useState(false);
  const [pastedConfig, setPastedConfig] = useState("");

  const selectedProfile = useMemo(
    () => profiles.find((profile) => profile.id === selectedId) ?? profiles[0],
    [profiles, selectedId],
  );

  useEffect(() => {
    void refreshProfiles();
  }, []);

  async function refreshProfiles() {
    setBusy("load");
    setError("");
    try {
      const nextProfiles = await callCommand<Profile[]>("list_profiles");
      setProfiles(nextProfiles);
      setSelectedId((current) => {
        if (nextProfiles.some((profile) => profile.id === current)) {
          return current;
        }
        return nextProfiles[0]?.id ?? "";
      });
      setNotice(nextProfiles.length ? "配置已载入" : "等待导入配置");
    } catch (err) {
      setError(readError(err));
    } finally {
      setBusy(null);
    }
  }

  async function importFromBrowserFile(file: File) {
    setBusy("import");
    setError("");
    try {
      const contents = await file.text();
      await importConfigSource(contents);
    } catch (err) {
      setError(readError(err));
    } finally {
      setBusy(null);
    }
  }

  async function importFromPastedConfig() {
    const contents = pastedConfig.trim();
    if (!contents) {
      setError("请先粘贴 JSON 配置。");
      return;
    }

    setBusy("import");
    setError("");
    try {
      await importConfigSource(contents);
      setPastedConfig("");
    } catch (err) {
      setError(readError(err));
    } finally {
      setBusy(null);
    }
  }

  async function importConfigSource(contents: string) {
    const profile = await callCommand<Profile>("import_config", {
      file: contents,
    });
    await refreshProfilesAfterImport(profile);
  }

  async function refreshProfilesAfterImport(profile: Profile) {
    const nextProfiles = await callCommand<Profile[]>("list_profiles");
    setProfiles(nextProfiles);
    setSelectedId(profile.id);
    setDiagnostic(null);
    setBenchmark(null);
    setNotice(`${profile.displayName} 已导入，密码已写入系统安全存储`);
  }

  async function runProfileCommand(
    command: "mount_profile" | "unmount_profile",
    action: BusyAction,
    success: (profile: Profile) => string,
  ) {
    if (!selectedProfile) return;
    setBusy(action);
    setError("");
    try {
      const profile = await callCommand<Profile>(command, {
        profileId: selectedProfile.id,
      });
      upsertProfile(profile);
      setNotice(success(profile));
    } catch (err) {
      setError(readError(err));
    } finally {
      setBusy(null);
    }
  }

  async function openSelectedMount() {
    if (!selectedProfile) return;
    setBusy("open");
    setError("");
    try {
      await callCommand("open_mount", { profileId: selectedProfile.id });
      setNotice("目录已打开");
    } catch (err) {
      setError(readError(err));
    } finally {
      setBusy(null);
    }
  }

  async function runDiagnostic() {
    if (!selectedProfile) return;
    setBusy("diagnose");
    setError("");
    try {
      const result = await callCommand<DiagnosticResult>("test_connection", {
        profileId: selectedProfile.id,
      });
      setDiagnostic(result);
      upsertProfile(result.profile);
      setNotice("诊断完成");
    } catch (err) {
      setError(readError(err));
    } finally {
      setBusy(null);
    }
  }

  async function runBenchmark() {
    if (!selectedProfile) return;
    setBusy("benchmark");
    setError("");
    try {
      const result = await callCommand<BenchmarkResult>("benchmark", {
        profileId: selectedProfile.id,
      });
      setBenchmark(result);
      upsertProfile(result.profile);
      setNotice("测速完成");
    } catch (err) {
      setError(readError(err));
    } finally {
      setBusy(null);
    }
  }

  function upsertProfile(profile: Profile) {
    setProfiles((current) =>
      current.map((item) => (item.id === profile.id ? profile : item)),
    );
  }

  function handleDrop(event: React.DragEvent<HTMLDivElement>) {
    event.preventDefault();
    setIsDragging(false);
    const file = event.dataTransfer.files.item(0);
    if (file) {
      void importFromBrowserFile(file);
    }
  }

  const actionDisabled = busy !== null || !selectedProfile;

  return (
    <main className="app-shell">
      <section className="topbar">
        <div className="brand-block">
          <div className="brand-mark">
            <img src={mountmateIcon} alt="" />
          </div>
          <div>
            <h1>MountMate</h1>
            <p>SMB 共享盘挂载助手</p>
          </div>
        </div>
        <button
          className="icon-button"
          type="button"
          title="刷新状态"
          disabled={busy !== null}
          onClick={() => void refreshProfiles()}
        >
          <RefreshCw size={18} />
        </button>
      </section>

      <section className="workspace">
        <div className="left-column">
          <div
            className={`import-zone ${isDragging ? "dragging" : ""}`}
            onDragOver={(event) => {
              event.preventDefault();
              setIsDragging(true);
            }}
            onDragLeave={() => setIsDragging(false)}
            onDrop={handleDrop}
          >
            <input
              ref={fileInputRef}
              className="hidden-input"
              type="file"
              accept="application/json,.json"
              onChange={(event) => {
                const file = event.currentTarget.files?.item(0);
                if (file) {
                  void importFromBrowserFile(file);
                }
                event.currentTarget.value = "";
              }}
            />
            <Import size={20} />
            <div>
              <strong>导入配置包</strong>
            </div>
            <textarea
              className="config-paste"
              value={pastedConfig}
              placeholder="粘贴 JSON 配置"
              spellCheck={false}
              onChange={(event) => setPastedConfig(event.currentTarget.value)}
            />
            <div className="import-actions">
              <button
                type="button"
                className="secondary-button"
                disabled={busy !== null}
                onClick={() => fileInputRef.current?.click()}
              >
                <Import size={16} />
                选择文件
              </button>
              <button
                type="button"
                className="secondary-button"
                disabled={busy !== null || !pastedConfig.trim()}
                onClick={() => void importFromPastedConfig()}
              >
                <ClipboardPaste size={16} />
                导入粘贴内容
              </button>
            </div>
          </div>

          <div className="profile-list">
            {profiles.length === 0 ? (
              <div className="empty-state">
                <Server size={22} />
                <span>还没有共享盘配置。</span>
              </div>
            ) : (
              profiles.map((profile) => (
                <button
                  key={profile.id}
                  type="button"
                  className={`profile-card ${
                    selectedProfile?.id === profile.id ? "selected" : ""
                  }`}
                  onClick={() => {
                    setSelectedId(profile.id);
                    setDiagnostic(null);
                    setBenchmark(null);
                  }}
                >
                  <div className="profile-mainline">
                    <span>{profile.displayName}</span>
                    <StatusPill mounted={profile.isMounted} />
                  </div>
                  <div className="meta-grid">
                    <span>{profile.server}</span>
                    <span>{profile.share}</span>
                    <span>{profile.mountPoint}</span>
                  </div>
                </button>
              ))
            )}
          </div>
        </div>

        <div className="right-column">
          <section className="panel active-profile">
            {selectedProfile ? (
              <>
                <div className="panel-header">
                  <div>
                    <h2>{selectedProfile.displayName}</h2>
                    <p>{selectedProfile.uncPath}</p>
                  </div>
                  <StatusPill mounted={selectedProfile.isMounted} />
                </div>

                <div className="facts-grid">
                  <Fact icon={<Server size={16} />} label="服务器" value={selectedProfile.server} />
                  <Fact icon={<HardDrive size={16} />} label="共享名" value={selectedProfile.share} />
                  <Fact icon={<KeyRound size={16} />} label="账号" value={selectedProfile.username} />
                  <Fact icon={<FolderOpen size={16} />} label="挂载点" value={selectedProfile.mountPoint} />
                </div>

                <div className="command-row">
                  <button
                    type="button"
                    className="primary-button"
                    disabled={actionDisabled || selectedProfile.isMounted}
                    onClick={() =>
                      void runProfileCommand(
                        "mount_profile",
                        "mount",
                        (profile) => `${profile.displayName} 已挂载`,
                      )
                    }
                  >
                    <PlugZap size={17} />
                    挂载
                  </button>
                  <button
                    type="button"
                    className="secondary-button"
                    disabled={actionDisabled || !selectedProfile.isMounted}
                    onClick={() =>
                      void runProfileCommand(
                        "unmount_profile",
                        "unmount",
                        (profile) => `${profile.displayName} 已断开`,
                      )
                    }
                  >
                    <Unplug size={17} />
                    断开
                  </button>
                  <button
                    type="button"
                    className="secondary-button"
                    disabled={actionDisabled || !selectedProfile.isMounted}
                    onClick={() => void openSelectedMount()}
                  >
                    <FolderOpen size={17} />
                    打开
                  </button>
                  <button
                    type="button"
                    className="secondary-button"
                    disabled={actionDisabled}
                    onClick={() => void runDiagnostic()}
                  >
                    <Activity size={17} />
                    诊断
                  </button>
                  <button
                    type="button"
                    className="secondary-button"
                    disabled={actionDisabled || !selectedProfile.isMounted}
                    onClick={() => void runBenchmark()}
                  >
                    <Gauge size={17} />
                    测速
                  </button>
                </div>
              </>
            ) : (
              <div className="empty-panel">
                <HardDrive size={24} />
                <span>导入配置后即可挂载。</span>
              </div>
            )}
          </section>

          <section className="status-strip" aria-live="polite">
            <span className={error ? "status-error" : "status-ok"}>
              {error || notice}
            </span>
            {busy ? <span className="busy-text">{busyLabel(busy)}</span> : null}
          </section>

          <section className="panel">
            <div className="panel-header compact">
              <div>
                <h2>诊断</h2>
                <p>网络、凭据、共享访问、挂载状态</p>
              </div>
              <ShieldCheck size={19} />
            </div>
            {diagnostic ? (
              <div className="check-list">
                {diagnostic.checks.map((check) => (
                  <div className="check-row" key={`${check.name}-${check.message}`}>
                    <StatusIcon status={check.status} />
                    <div>
                      <strong>{check.name}</strong>
                      <span>{check.message}</span>
                    </div>
                  </div>
                ))}
                {diagnostic.suggestions.length ? (
                  <div className="suggestions">
                    {diagnostic.suggestions.map((suggestion) => (
                      <div key={suggestion}>
                        <AlertTriangle size={15} />
                        <span>{suggestion}</span>
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            ) : (
              <div className="placeholder-line">点击诊断后显示结果。</div>
            )}
          </section>

          <section className="panel">
            <div className="panel-header compact">
              <div>
                <h2>测速</h2>
                <p>大文件吞吐与小文件写入</p>
              </div>
              <Gauge size={19} />
            </div>
            {benchmark ? (
              <div className="metric-grid">
                <Metric label="大文件写入" value={`${formatNumber(benchmark.largeWriteMbps)} MB/s`} />
                <Metric label="大文件读取" value={`${formatNumber(benchmark.largeReadMbps)} MB/s`} />
                <Metric label="小文件写入" value={`${formatNumber(benchmark.smallFilesPerSecond)} 个/s`} />
                <Metric label="测试目录" value={benchmark.mountPoint} />
              </div>
            ) : (
              <div className="placeholder-line">挂载后点击测速。</div>
            )}
          </section>
        </div>
      </section>
    </main>
  );
}

function Fact({
  icon,
  label,
  value,
}: {
  icon: ReactNode;
  label: string;
  value: string;
}) {
  return (
    <div className="fact">
      {icon}
      <span>{label}</span>
      <strong>{value || "-"}</strong>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="metric">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function StatusPill({ mounted }: { mounted: boolean }) {
  return (
    <span className={`status-pill ${mounted ? "mounted" : "idle"}`}>
      {mounted ? "已挂载" : "未挂载"}
    </span>
  );
}

function StatusIcon({ status }: { status: CheckStatus }) {
  if (status === "ok") {
    return <CheckCircle2 className="check-icon ok" size={18} />;
  }
  if (status === "warning") {
    return <AlertTriangle className="check-icon warning" size={18} />;
  }
  return <XCircle className="check-icon error" size={18} />;
}

function busyLabel(busy: BusyAction) {
  switch (busy) {
    case "load":
      return "刷新中";
    case "import":
      return "导入中";
    case "mount":
      return "挂载中";
    case "unmount":
      return "断开中";
    case "open":
      return "打开中";
    case "diagnose":
      return "诊断中";
    case "benchmark":
      return "测速中";
    default:
      return "";
  }
}

function formatNumber(value: number) {
  return new Intl.NumberFormat("zh-CN", {
    maximumFractionDigits: 2,
    minimumFractionDigits: 0,
  }).format(value);
}

function readError(error: unknown) {
  if (typeof error === "string") {
    return error;
  }
  if (error instanceof Error) {
    return error.message;
  }
  return "操作失败。";
}

type CommandArgs = Record<string, unknown> | undefined;

let previewProfiles: Profile[] = [];

async function callCommand<T>(command: string, args?: CommandArgs): Promise<T> {
  if (isTauriRuntime()) {
    return invoke<T>(command, args);
  }
  return previewCommand<T>(command, args);
}

function isTauriRuntime() {
  const windowValue = window as unknown as Record<string, unknown>;
  return Boolean(windowValue.__TAURI_INTERNALS__ || windowValue.__TAURI__);
}

async function previewCommand<T>(command: string, args?: CommandArgs): Promise<T> {
  switch (command) {
    case "list_profiles":
      return structuredClone(previewProfiles) as T;
    case "import_config": {
      const file = String(args?.file ?? "");
      if (!file.trimStart().startsWith("{")) {
        throw new Error("浏览器预览不能读取系统路径，请拖入或粘贴 JSON 配置。");
      }
      const config = JSON.parse(file) as {
        displayName: string;
        server: string;
        share: string;
        domain?: string;
        username: string;
        defaultDriveLetter?: string;
        mountName?: string;
      };
      const profile = makePreviewProfile(config);
      previewProfiles = [
        profile,
        ...previewProfiles.filter((item) => item.id !== profile.id),
      ];
      return structuredClone(profile) as T;
    }
    case "mount_profile":
    case "unmount_profile": {
      const id = String(args?.profileId ?? "");
      const profile = previewProfiles.find((item) => item.id === id);
      if (!profile) throw new Error("未找到该共享盘配置。");
      profile.isMounted = command === "mount_profile";
      return structuredClone(profile) as T;
    }
    case "open_mount":
      return undefined as T;
    case "test_connection": {
      const profile = findPreviewProfile(args);
      return {
        profile,
        checks: [
          {
            name: "SMB 端口",
            status: "warning",
            message: "浏览器预览模式不执行真实网络探测。",
          },
          {
            name: "凭据",
            status: "ok",
            message: "配置包已通过预览解析。",
          },
          {
            name: "挂载状态",
            status: profile.isMounted ? "ok" : "warning",
            message: profile.isMounted ? `已挂载：${profile.mountPoint}` : "当前未挂载。",
          },
        ],
        suggestions: ["在 Tauri 桌面应用中运行可执行真实 SMB 诊断。"],
      } as T;
    }
    case "benchmark": {
      const profile = findPreviewProfile(args);
      if (!profile.isMounted) throw new Error("共享盘尚未挂载，无法测速。");
      return {
        profile,
        mountPoint: profile.mountPoint,
        largeFileBytes: 8388608,
        largeWriteSeconds: 0.65,
        largeWriteMbps: 12.3,
        largeReadSeconds: 0.58,
        largeReadMbps: 13.8,
        smallFileCount: 20,
        smallFileBytesEach: 4096,
        smallWriteSeconds: 5.2,
        smallFilesPerSecond: 3.85,
      } as T;
    }
    default:
      throw new Error(`预览模式尚未实现命令：${command}`);
  }
}

function makePreviewProfile(config: {
  displayName: string;
  server: string;
  share: string;
  domain?: string;
  username: string;
  defaultDriveLetter?: string;
  mountName?: string;
}): Profile {
  const share = config.share || "CompanyDisk";
  const mountName = config.mountName || share;
  const id = `${config.server}-${share}-${config.username}`;
  return {
    id,
    displayName: config.displayName || share,
    server: config.server,
    share,
    domain: config.domain ?? "",
    username: config.username,
    defaultDriveLetter: config.defaultDriveLetter || "Z",
    mountName,
    smbUrl: `smb://${config.server}/${share}`,
    uncPath: `\\\\${config.server}\\${share}`,
    mountPoint: navigator.userAgent.includes("Mac")
      ? `/Volumes/${mountName}`
      : `${config.defaultDriveLetter || "Z"}:\\`,
    isMounted: false,
    hasPassword: true,
  };
}

function findPreviewProfile(args?: CommandArgs) {
  const id = String(args?.profileId ?? "");
  const profile = previewProfiles.find((item) => item.id === id);
  if (!profile) throw new Error("未找到该共享盘配置。");
  return structuredClone(profile);
}

export default App;
