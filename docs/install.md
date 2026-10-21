# Installing desigma

Download the file for your platform from the
[latest release](https://github.com/deepfounder-ai/desigma/releases):

| Platform | File |
|---|---|
| macOS (Apple Silicon and Intel) | `desigma-<version>-macos.dmg` (one universal file for both) |
| Windows (x64) | `desigma-<version>-x86_64-pc-windows-msvc.zip` |
| Linux (x64) | `desigma-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| macOS, for scripts and CLI use | `desigma-<version>-macos-universal.tar.gz` |

The macOS disk image holds `desigma.app` (the editor, with the `desigma-diff` tool inside). The
Windows and Linux archives hold the `desigma` editor, the `desigma-diff` tool, the licenses and the
README. `SHA256SUMS` on the release page lets you check the download:

```sh
sha256sum -c SHA256SUMS --ignore-missing      # Linux
shasum -a 256 -c SHA256SUMS --ignore-missing  # macOS
```

## The first-launch warning

desigma's release builds are **not code-signed yet**, so Windows and macOS warn
the first time you open them. The warning is about the missing publisher
signature, not about anything the app does: the source is public and every
release is built by GitHub Actions from a tagged commit.

### Windows

1. Unzip the archive and double-click `desigma.exe` (it carries the desigma icon and version info).
2. SmartScreen shows **"Windows protected your PC"**. Click **More info**.
3. Click **Run anyway**. Windows remembers the choice for this file.

### macOS

One `.dmg` works on both Apple Silicon and Intel Macs (macOS 11 or newer).

1. Download `desigma-<version>-macos.dmg` from the release page.
2. Double-click the `.dmg` to open it.
3. Drag **desigma** onto the **Applications** shortcut in that window.
4. Open **Applications** and double-click **desigma** (you can eject the disk image now).
5. The first time, macOS says it cannot verify that desigma is free of malware. Click **Done**.
6. Open **System Settings → Privacy & Security** and scroll down to the message that
   "desigma" was blocked. Click **Open Anyway**.
7. Confirm with your password or Touch ID, then click **Open** in the last dialog.

That is only needed once; afterwards desigma opens like any other app. The same steps are
in `Read me first.txt` inside the disk image.

If you prefer Terminal, this replaces steps 5 to 7:

```sh
xattr -dr com.apple.quarantine /Applications/desigma.app
```

The app is ad-hoc signed but not notarized by Apple yet, which is why the one-time confirmation
is needed.

**Command-line tool.** `desigma-diff` lives inside the app:

```sh
/Applications/desigma.app/Contents/MacOS/desigma-diff --help
```

**Opening files.** `.desigma` and `.fig` files list desigma under **Open With** in Finder.
Double-clicking a file in Finder does not hand it to a running desigma yet (the windowing
library does not expose macOS "open file" events); use **File → Open**, drag the file onto the
window, or run `/Applications/desigma.app/Contents/MacOS/desigma path/to/file.desigma`.

`desigma-<version>-macos-universal.tar.gz` holds the same `desigma.app` plus the licenses, for
scripting; unpack it and run `xattr -dr com.apple.quarantine desigma.app` once.

### Linux

No warning. Unpack, make sure the system libraries are installed (most desktops
have them already), and run it:

```sh
tar -xzf desigma-<version>-x86_64-unknown-linux-gnu.tar.gz
cd desigma-<version>-x86_64-unknown-linux-gnu
./desigma
```

To add desigma to your application menu with its icon, run the bundled installer. It needs no
sudo and installs under `~/.local` (binary, `.desktop` entry, icons and file types):

```sh
./install.sh
```

On a minimal system install the libraries the editor links against, for
example on Debian or Ubuntu:

```sh
sudo apt install libxkbcommon0 libwayland-client0 libx11-6 libxcursor1 libxrandr2 libxi6 libgl1 libfontconfig1
```

## GPU rendering

The canvas draws with the GPU (Vulkan, Metal, DirectX 12 or OpenGL) and falls
back to the CPU renderer when no suitable adapter is found. If you see drawing
problems, turn off **Preferences → Use GPU rendering**.

## Updates

A few seconds after launch, and then once a day while it runs, desigma asks GitHub's
release list (one request to `api.github.com`, with the user agent `desigma/<version>`)
whether a newer version is out. If there is one, a banner offers the release notes and a
**Download** button that opens the release page; nothing is downloaded or installed on its
own. **Skip this version** stops the reminders until the next release.

Turn the automatic check off with **Preferences → Check for updates automatically**, or set
`DESIGMA_NO_UPDATE_CHECK=1`. **Help → Check for updates…** still works. Builds from source
never check.

## Building from source

```sh
git clone https://github.com/deepfounder-ai/desigma
cd desigma
cargo run --release -p desigma-app
```

The pinned Rust toolchain installs automatically through `rust-toolchain.toml`.
See [CONTRIBUTING.md](../CONTRIBUTING.md) for the Linux build packages and the
test workflow.

## Signed builds

Signing is planned: Windows binaries through a code-signing certificate, the macOS
app signed with a Developer ID and notarized. Once that lands, these warnings go away and
this page will say so.
