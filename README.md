# MountMate

MountMate is a Tauri v2 desktop helper for one-click SMB mounting on Windows and macOS.

The first version focuses on mounting only: import a JSON config package, store the password in the system credential store, mount or unmount the SMB share, open the mounted directory, run diagnostics, and run a small benchmark.

## Features

- Windows SMB mapping through `net use`, defaulting to `Z:`.
- macOS SMB mounting through the system SMB/Finder mount flow, defaulting to `/Volumes/<mountName>`.
- Passwords are accepted from an import package but are not written to the local profile JSON.
- Diagnostics cover TCP 445 reachability, credential availability, share access, and mount state.
- Benchmark writes and reads temporary files inside the mounted share, then cleans them up.

## Config Package

Copy `examples/companydisk.mountmate.json`, replace the password before importing, and do not commit the filled file.

```json
{
  "schemaVersion": 1,
  "displayName": "CompanyDisk",
  "server": "192.168.77.174",
  "share": "CompanyDisk",
  "domain": "",
  "username": "yaoyelan",
  "password": "REPLACE_BEFORE_IMPORT",
  "defaultDriveLetter": "Z",
  "mountName": "CompanyDisk"
}
```

After import, MountMate stores the password in Keychain on macOS or Credential Manager on Windows. The app profile file only keeps non-secret metadata.

## Development

```bash
pnpm install
pnpm tauri dev
```

Useful checks:

```bash
pnpm build
cd src-tauri && cargo test
```

## Release Builds

```bash
pnpm tauri build
```

GitHub Actions builds Windows and macOS artifacts. Signing variables are optional; if they are missing, the workflow still produces unsigned installers.

macOS signing and notarization variables:

- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`
- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`

Windows signing variables:

- `WINDOWS_CERTIFICATE`
- `WINDOWS_CERTIFICATE_PASSWORD`

## Platform Notes

Windows mounts with:

```powershell
net use Z: \\server\share /user:username * /persistent:yes
```

The password is sent to the command through stdin, not embedded in the command arguments.

macOS mounts through AppleScript's `mount volume` command and unmounts with:

```bash
diskutil unmount /Volumes/CompanyDisk
```

For predictable macOS mount paths, keep `mountName` the same as the SMB share name.

## License

MIT
