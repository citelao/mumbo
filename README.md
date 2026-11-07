# Mumbo

This is still in development. I wanted to learn Tauri & I wanted a Mumble client that was auto-configured with good audio.

## Development

```bash
  brew install cmake # needed for opus
  npm install
  npm run tauri android init
  npm run tauri ios init

# For Desktop development, run:
  npm run tauri dev

# For Android development, run:
  npm run tauri android dev

# For iOS development, run:
  npm run tauri ios dev
```

Host a local Mumble server with Podman:

```bash
podman machine start # etc...
npm run mumble:up
npm run mumble:logs # for logs
npm run mumble:down # shut it down later
```

Connect with `localhost:64738`, password `test` by default. See `dev/mumble-server/compose.yaml`.

### Distribution

```bash
npm run tauri build
```

#### Flatpak

The flatpak is built from source against the GNOME runtime (see `packaging/flatpak/`):

```bash
flatpak-builder --user --install-deps-from=flathub --force-clean --install \
  flatpak-build packaging/flatpak/com.citelao.mumbo.yml
flatpak run com.citelao.mumbo
```

To run a build without installing it, use `flatpak-builder --run flatpak-build packaging/flatpak/com.citelao.mumbo.yml env WAYLAND_DISPLAY=$WAYLAND_DISPLAY mumbo`. `--run` mounts the Wayland socket but doesn't set `WAYLAND_DISPLAY`, so GTK fails to initialize without it.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)