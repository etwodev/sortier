# Sortier

A desktop app for organising, tagging, searching and reading **sequenced images**: multi-page art,
manga chapters, pixiv-style `_p0/_p1` posts, comics, webtoon strips, plus GIFs and videos.
GPUI draws the interface; wgpu draws the pictures. The layout follows Spline's: a title bar, the
menus and workspace tabs, and Blender-style areas you can resize, split and join (right-click a seam).

## Run

On Windows, build FFmpeg's static libraries once first. It uses vcpkg (from the PATH, or Visual Studio's C++
build tools), takes 10 to 30 minutes the first time, and fills the git-ignored `.vcpkg` folder that
`.cargo/config.toml` points the build at:

```bash
powershell -ExecutionPolicy Bypass -File scripts/setup-ffmpeg.ps1
```

On macOS and Linux, Cargo builds FFmpeg from source itself (it needs `make`, `nasm` and a C compiler).
Everywhere, generating FFmpeg's bindings needs libclang (Visual Studio's C++ build tools include it).

```bash
cargo run --release -p sortier-app
```

Pass folders to add them (`cargo run --release -p sortier-app -- "D:\Art"`), use File › Add Folder,
or drop folders on the window. Data lives in `%LOCALAPPDATA%\sortier` (`library.json`,
`workspaces.json`, thumbnails); set `SORTIER_DATA_DIR` to use another folder. Files on disk are never
moved or changed.

## Editors and workspaces

| Editor | What it does |
|---|---|
| Library | The results as tiles. Search, tag the selection, sort and tile options. A/D/W/S or arrows move, Enter opens. |
| Viewer | The open page drawn by wgpu: fit, fit width/height, 1:1, zoom and pan, two-page spread, right to left, animated GIF/PNG/WebP, video with sound. A/D turn pages and carry on into the next item; W/S jump items. |
| Pages | The open sequence's pages; drag to reorder, make a cover, take a page out. |
| Browser | Quick filters, folders, and tags grouped by namespace. Click a tag to require it, Alt-click to exclude it. |
| Inspector | The selection's file details, sequence name, tags, and the open page's own tags. |

Workspaces **Library**, **Reader** and **Tagging** start set up; add, rename or rearrange them from the tabs.
Help › Keyboard Shortcuts (F1) lists every key.

Search: `cat dog`, `-sketch`, `cat|dog`, `artist:*`, `type:video|gif|image`, `is:untagged`, `is:seq`,
`name:…`, `seq:…`, `ext:…`.

Videos: FFmpeg's libraries are linked into Sortier, so nothing needs installing to play them.
Videos decode on the graphics card when it can (Direct3D 11 or DXVA 2 on Windows, VideoToolbox on macOS,
VA-API on Linux); codecs the card can't decode fall back to the processor. The badge in the video controls says
which is in use, and View › Decode Video on the GPU turns it off.

Vaults: each vault is a library of its own (folders, tags, sequences, collections). The vault menu in the middle
of the title bar switches, makes, renames and removes them, and connects to a library hosted with Sortier Remote.

## Hosting a library (Sortier Remote)

Sortier Remote serves a library and its files to Sortier on other computers, which can browse, read, play, tag
and upload to it. It scans its folders and makes the thumbnails itself, and keeps everyone's edits in step.

```bash
cargo build --release -p sortier-remote
```

That makes `target/release/sortier-remote.exe`, the command, and on Windows `sortier-remote-tray.exe`, which runs
the server from the notification area. Keep the two together wherever you put them: the tray app runs the server
with `sortier-remote.exe`.

### Set it up

On the computer with the files, once:

```bash
sortier-remote set-password
```

```bash
sortier-remote add-folder "D:\Art"
```

The password needs at least 8 characters (`SORTIER_REMOTE_PASSWORD` gives it without a prompt). Add as many
folders as you like. Optionally, `sortier-remote set-upload-folder "D:\Art\Uploads"` picks where uploads go (a
folder inside a served one; otherwise the first folder).

| Command | What it does |
|---|---|
| `serve` | Runs the server in the terminal. |
| `set-password` | Sets the password devices log in with. |
| `add-folder <path>`, `remove-folder <path>`, `folders` | Changes or lists the folders served; a running server notices at its next scan. Files are never touched. |
| `set-upload-folder <path>` | Where uploads go. |
| `fingerprint` | Prints the certificate fingerprint devices check when connecting. |
| `devices`, `forget-devices` | Lists the devices logged in, or logs them all out (restart the server afterwards). |

Everything lives in the data folder, `%LOCALAPPDATA%\sortier-remote` (`~/.local/share/sortier-remote` on Linux,
`~/Library/Application Support/sortier-remote` on macOS): `config.toml`, `library.json`, thumbnails, the
certificate and its key, the devices logged in, and the tray app's `sortier-remote.log`. Pass `--data <folder>`
(or set `SORTIER_REMOTE_DIR`) to use another, and use the same one for every command. `config.toml` takes:

| Setting | Default | |
|---|---|---|
| `port` | `7461` | The port to listen on. |
| `bind` | `"0.0.0.0"` | Every network; `"127.0.0.1"` for this computer only. |
| `rescan_minutes` | `10` | How often the folders are looked through for files that came or went. |
| `upload_folder` | the first folder | Set with `set-upload-folder`. |

Restart the server after changing it.

### Keep it running

**Windows, while you're signed in:** run `sortier-remote-tray.exe` and choose **Start with Windows** from its
menu. Its heart sits in the notification area; the menu shows whether it's listening and has Show Fingerprint,
Open Data Folder, Open Log, Restart Server and Quit. Windows tucks new icons behind the ^ arrow: drag the heart
onto the taskbar to keep it in view. Start with Windows adds a "Sortier Remote" entry to your user's Run key (no
administrator needed), so it also shows in Task Manager's Startup apps; untick it to remove it. Give the tray app
`--data <folder>` if the server's data isn't in the usual place, and it remembers that for Start with Windows.

The tray app runs the server as a process of its own (`sortier-remote serve`, with no window), so a crash in the
server doesn't take the tray with it. It writes everything the server prints to `sortier-remote.log`, and if the
server crashes it notes why (heap corruption, say) and starts it again five seconds later; the menu then reads
"restarted after a crash at …". A server that crashes three times in ten minutes is left stopped, with a message
saying so. When the server can't start at all (no password set, the port in use), a message says why; put it
right and choose Restart Server.

**Windows, before anyone signs in:** have Task Scheduler start the command at boot as SYSTEM. In PowerShell as
administrator, with your own paths:

```powershell
$action = New-ScheduledTaskAction -Execute 'C:\Sortier\sortier-remote.exe' -Argument 'serve --data C:\SortierRemote'
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName 'Sortier Remote' -Action $action -Trigger (New-ScheduledTaskTrigger -AtStartup) -Settings $settings -Principal $principal
```

The time limit of zero matters: tasks are otherwise stopped after three days. Run the setup commands with the
same `--data C:\SortierRemote`, and serve folders on local drives (SYSTEM doesn't see mapped drive letters).
Use this or the tray app for a data folder, not both. (`sc.exe create` doesn't work: `sortier-remote` isn't a
Windows service program.)

**Linux:** a systemd unit, enabled with `sudo systemctl enable --now sortier-remote`:

```ini
# /etc/systemd/system/sortier-remote.service
[Unit]
Description=Sortier Remote
After=network-online.target
Wants=network-online.target

[Service]
User=you
ExecStart=/usr/local/bin/sortier-remote serve
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

**macOS:** a launchd agent, `~/Library/LaunchAgents/sortier-remote.plist`, loaded with
`launchctl load ~/Library/LaunchAgents/sortier-remote.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>sortier-remote</string>
  <key>ProgramArguments</key>
  <array><string>/usr/local/bin/sortier-remote</string><string>serve</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
```

### Connect

In Sortier, open the vault menu and choose **Connect to Sortier Remote…**. Give the vault a name, the server's
address (`192.168.1.20`, or `host:port` if the port isn't 7461) and the password. Sortier then shows the server's
fingerprint: check it matches `sortier-remote fingerprint` (or the tray's Show Fingerprint) before choosing
**Trust and Log In**. The vault menu's Cache Size sets how much of a remote vault is kept on this computer.

Other computers can only reach the server if the firewall lets them. Windows asks the first time the server
listens; to allow it yourself, in PowerShell as administrator:

```powershell
New-NetFirewallRule -DisplayName 'Sortier Remote' -Direction Inbound -Protocol TCP -LocalPort 7461 -Action Allow -Profile Private
```

To use it away from home, prefer a VPN such as Tailscale or WireGuard to opening the port on your router: the
server is protected by its password and a self-made certificate, nothing more.

## Crates

| Crate | Purpose |
|---|---|
| `sortier-app` | The window shell, area host, workspace tabs, menus, the shared `LibraryModel`, and the editors. |
| `sortier-ui` | Design tokens, theme and UI primitives, ported from Spline's `spline-ui` (the only crate using gpui-component). |
| `sortier-areas` | The area layout tree (Spline's `spline-areas`). |
| `sortier-library` | Items, sequences, tags, search, scanning and the thumbnail cache. No UI. |
| `sortier-render` | The wgpu renderer: textures with mipmaps (tiled past the GPU's size limit), views read back as BGRA frames. No UI. |
| `sortier-media` | Decoding stills, animations, documents and video (FFmpeg's libraries, linked statically) and playing sound (cpal). No UI. |
| `sortier-client` | Talking to Sortier Remote: the protocol, an HTTPS client that trusts the server by its fingerprint, and the disk cache. |
| `sortier-remote` | Sortier Remote: the `sortier-remote` command and, on Windows, the `sortier-remote-tray` app. |

```bash
cargo test --workspace      # sortier-render's tests need a GPU
cargo clippy --workspace --all-targets
```
