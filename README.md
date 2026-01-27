# mpvpaper-rs

A video wallpaper player using mpv for wlroots-based Wayland compositors.

Rust port of [mpvpaper](https://github.com/GhostNaN/mpvpaper).

## Features

- Video wallpaper playback using libmpv
- Multi-monitor support (individual or all outputs)
- Auto-pause/stop when windows are fullscreen
- Slideshow mode for playlists
- Fork mode for background execution
- Pauselist/stoplist for app-specific behavior
- Layer shell support (background, bottom, top, overlay)

## Requirements

- Wayland compositor with wlr-layer-shell support (Hyprland, Sway, etc.)
- EGL/OpenGL support
- Rust toolchain (1.70+)

### System Dependencies

**Arch Linux / EndeavourOS:**
```bash
sudo pacman -S mpv egl wayland
```

**Debian / Ubuntu:**
```bash
sudo apt install libmpv-dev libegl1-mesa-dev libwayland-dev
```

**Fedora:**
```bash
sudo dnf install mpv-devel mesa-libEGL-devel wayland-devel
```

## Installation

### Method 1: Install from local repository (Recommended)

```bash
# Clone the repository
git clone https://github.com/BitYoungjae/mpvpaper-rs.git
cd mpvpaper-rs

# Install directly to ~/.cargo/bin/
cargo install --path crates/mpvpaper-rs
```

### Method 2: Build manually

```bash
# Build release binaries
cargo build --release

# (Optional) Install to system
sudo install -m 755 target/release/mpvpaper-rs /usr/local/bin/
sudo install -m 755 target/release/mpvpaper-rs-holder /usr/local/bin/
```

After installation, binaries will be available:
- `mpvpaper-rs` - Main application
- `mpvpaper-rs-holder` - Helper for auto-stop recovery

**Note:** Make sure `~/.cargo/bin` is in your `PATH` (it usually is if you use Rust).

## Usage

### List available outputs

```bash
mpvpaper-rs -d
```

### Play video on a specific output

```bash
mpvpaper-rs DP-2 /path/to/video.mp4
```

### Play video on all outputs

```bash
mpvpaper-rs ALL /path/to/video.mp4
# or
mpvpaper-rs '*' /path/to/video.mp4
```

### Auto-pause when fullscreen window detected

```bash
mpvpaper-rs -p DP-2 /path/to/video.mp4
```

### Auto-stop when fullscreen window detected

```bash
mpvpaper-rs -s DP-2 /path/to/video.mp4
```

#### Auto-pause vs Auto-stop

| Feature | Auto-pause (`-p`) | Auto-stop (`-s`) |
|---------|-------------------|------------------|
| Process | Main process keeps running | Switches to holder process |
| Memory | Full MPV context in memory | Minimal resources |
| Resume | Instant resume | Restores from saved position |
| Use case | Short fullscreen sessions | Long gaming/video sessions |

### Fork to background

```bash
mpvpaper-rs -f DP-2 /path/to/video.mp4
```

### Slideshow mode

```bash
mpvpaper-rs -n 5 DP-2 /path/to/video.mp4  # Change every 5 seconds
```

### Pass options to mpv

**Important:** Use `=` to pass options properly. The quotes prevent shell word splitting.

```bash
mpvpaper-rs -o="--loop --mute" DP-2 /path/to/video.mp4
# or
mpvpaper-rs --mpv-options="--loop --mute" DP-2 /path/to/video.mp4
```

#### Useful MPV Options

| Option | Description |
|--------|-------------|
| `--no-audio` | Disable audio playback |
| `--loop` | Loop video infinitely |
| `--gpu-api=vulkan` | Use Vulkan for rendering (better performance) |
| `--panscan=1` | Auto-adjust aspect ratio to fill screen |
| `--hwdec=auto` | Enable hardware decoding |

**Production example:**
```bash
mpvpaper-rs -p -o="--no-audio --loop --gpu-api=vulkan --panscan=1" ALL /path/to/video.mp4
```

### Verbose output

```bash
mpvpaper-rs -v DP-2 /path/to/video.mp4   # Verbose
mpvpaper-rs -vv DP-2 /path/to/video.mp4  # More verbose
```

## Options

| Option | Description |
|--------|-------------|
| `-d, --help-output` | Display available outputs and exit |
| `-f, --fork` | Fork to background |
| `-p, --auto-pause` | Pause when wallpaper is covered |
| `-s, --auto-stop` | Stop when wallpaper is covered |
| `-n, --slideshow <SEC>` | Slideshow interval in seconds |
| `-l, --layer <LAYER>` | Layer: background, bottom, top, overlay |
| `-o, --mpv-options <OPTS>` | Pass options to mpv |
| `-v, --verbose` | Increase verbosity (-v, -vv) |

### Output Selectors

The `<OUTPUT>` argument accepts:

| Selector | Description |
|----------|-------------|
| `DP-2`, `HDMI-A-1`, etc. | Specific output name (use `-d` to list) |
| `ALL` | All available outputs |
| `*` | Same as `ALL` (shell wildcard, must quote) |

## Configuration

Configuration files are stored in `~/.config/mpvpaper-rs/`:

- `pauselist` - List of apps that pause playback (one per line)
- `stoplist` - List of apps that stop playback (one per line)

## Real-World Usage Examples

Examples for using mpvpaper-rs with [Omarchy](https://omarchy.org) (Hyprland-based desktop environment).

### systemd Service

Create `~/.config/systemd/user/mpvpaper.service`:

```ini
[Unit]
Description=Motion wallpaper with mpvpaper-rs
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.local/bin/mpvpaper-rs -p -o="--no-audio --loop --gpu-api=vulkan --panscan=1" ALL %h/Backgrounds/Motions/current.mp4
# Optional: Kill static wallpaper (swaybg) before starting. Remove if not using swaybg.
ExecStartPre=-/bin/bash -c 'pkill -f swaybg || true'
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
```

Enable with:
```bash
systemctl --user enable --now mpvpaper.service
```

### Auto-Rotate Timer

Create `~/.config/systemd/user/mpvpaper-rotate.timer` to change wallpaper every 10 minutes:

```ini
[Unit]
Description=Timer to rotate motion wallpaper every 10 minutes

[Timer]
OnBootSec=0sec
OnUnitActiveSec=10min
AccuracySec=1s

[Install]
WantedBy=timers.target
```

### Hyprland Keybinding

Add to your Hyprland config to switch wallpapers with `SUPER + CTRL + SPACE`:

```conf
bindd = SUPER CTRL, SPACE, Next motion wallpaper, exec, ~/.local/bin/mpvpaper-switch && systemctl --user restart mpvpaper.service
```

### Video Switcher Script

Create `~/.local/bin/mpvpaper-switch` to cycle through videos:

```bash
#!/bin/bash
MOTIONS_DIR="$HOME/Backgrounds/Motions"
CURRENT_LINK="$MOTIONS_DIR/current.mp4"

# Find videos (excluding current.mp4 symlink)
mapfile -t VIDEOS < <(find "$MOTIONS_DIR" -maxdepth 1 -type f \( -name "*.mp4" -o -name "*.webm" -o -name "*.mkv" \) ! -name "current.mp4" | sort)

# Get current video index
CURRENT_VIDEO=$(readlink -f "$CURRENT_LINK" 2>/dev/null)
CURRENT_INDEX=-1
for i in "${!VIDEOS[@]}"; do
    [[ "${VIDEOS[$i]}" == "$CURRENT_VIDEO" ]] && CURRENT_INDEX=$i && break
done

# Select next video (circular)
NEXT_INDEX=$(( (CURRENT_INDEX + 1) % ${#VIDEOS[@]} ))
SELECTED="${VIDEOS[$NEXT_INDEX]}"

# Update symlink
ln -nsf "$SELECTED" "$CURRENT_LINK"

# Notify
BASENAME=$(basename "$SELECTED")
notify-send "Background switched" "${BASENAME%.*}" -t 1500
```

Make executable: `chmod +x ~/.local/bin/mpvpaper-switch`

## Signals

- `SIGUSR1` - Pause/unpause playback
- `SIGUSR2` - Stop playback

## License

GPL-3.0 - See [LICENSE](LICENSE) for details.

## Acknowledgments

Based on [mpvpaper](https://github.com/GhostNaN/mpvpaper) by GhostNaN.
