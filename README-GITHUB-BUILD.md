# Build ui-slint for Raspberry Pi ARM64 with GitHub Actions

This project contains two build variants:

- **desktop** — the default Slint/Winit build for a graphical desktop session.
- **kms** — the Slint Linux KMS build for a directly attached display (DRM/KMS).

The workflow builds both variants on a native ARM64 GitHub-hosted runner and publishes downloadable artifacts. It does not run the UI on physical Raspberry Pi hardware, so the KMS display, touch input, GPU/DRM driver and runtime permissions must still be tested on the Pi.

## 1. Create a GitHub repository

1. Sign in at [GitHub](https://github.com/).
2. Create a new repository, for example `ui-slint`. Public or private repositories can use the current standard ARM64 runner labels; private-repository usage is subject to your plan's Actions minutes.
3. Extract the supplied project ZIP on your computer.
4. Open the extracted `ui-slint` folder. It should contain `Cargo.toml`, `Cargo.lock`, `src/`, `ui/`, `.cargo/`, and this `.github/` directory.
5. Copy `README-GITHUB-BUILD.md` to `README.md` if you want it to be the repository's front page.
6. Upload the **contents** of the `ui-slint` folder to the root of the GitHub repository. Do not upload the outer folder as an extra nested directory.

You can upload files through GitHub's web interface, or use Git locally:

```bash
git init
git add .
git commit -m "Add ui-slint Raspberry Pi project and ARM64 build"
git branch -M main
git remote add origin https://github.com/YOUR-USERNAME/ui-slint.git
git push -u origin main
```

Replace `YOUR-USERNAME` and `ui-slint` with your actual GitHub account and repository name.

## 2. Run the build

1. Open the repository on GitHub.
2. Select **Actions**.
3. Select **Build Raspberry Pi ARM64** in the left sidebar.
4. Select **Run workflow**, keep the `main` branch selected, and click **Run workflow**.
5. Open the running workflow and wait for both matrix jobs: `desktop` and `kms`.

A push to `main` or `master` also starts a build automatically.

## 3. Download the executable

When the workflow finishes successfully:

1. Open the completed workflow run.
2. Scroll to **Artifacts** at the bottom.
3. Download one or both artifacts:
   - `ui-slint-desktop-aarch64`
   - `ui-slint-kms-aarch64`
4. Extract the downloaded artifact ZIP. It contains the executable and its SHA-256 checksum file.

The executable is named `ui-slint-desktop-aarch64` or `ui-slint-kms-aarch64` so you can distinguish the two builds.

## 4. Run it on Raspberry Pi

Copy the selected executable to the Pi, for example with `scp`:

```bash
scp ui-slint-kms-aarch64 rbd@RASPBERRY_PI_IP:~/ui-slint
```

On the Pi:

```bash
chmod +x ~/ui-slint/ui-slint-kms-aarch64
~/ui-slint/ui-slint-kms-aarch64
```

For the desktop build, use `ui-slint-desktop-aarch64` instead. The desktop variant requires a working graphical session. The KMS variant requires a supported DRM/KMS display setup and the appropriate device permissions. Do not run both variants at the same time against the same display.

The program also expects its companion frequency-monitor backend and its Unix socket to be available as configured by the application. Building the UI executable does not build or start that separate backend.

## 5. Create a versioned GitHub Release (optional)

After pushing your source, create and push a version tag:

```bash
git tag v0.7.0
git push origin v0.7.0
```

The workflow builds both variants and creates a GitHub Release containing the ARM64 executables and checksum files.

## Build details

- Runner: `ubuntu-22.04-arm` (native ARM64).
- Rust: stable toolchain.
- Target: runner's native `aarch64-unknown-linux-gnu` host.
- Desktop command: `cargo build --locked --release -p ui-slint`.
- KMS command: `cargo build --locked --release -p ui-slint --no-default-features --features kms`.
- The workflow installs Fontconfig, FreeType, X11/Wayland, DRM, GBM, libinput and related development packages before building.

The workflow deliberately does not pass `/home/jetex/pi-sysroot` or use cross-linker flags. It compiles natively for ARM64.

## Troubleshooting

- **Workflow cannot find `ubuntu-22.04-arm`:** confirm the repository has GitHub Actions enabled and retry. If the runner label is unavailable to your account, change it to `ubuntu-24.04-arm` in `.github/workflows/build-rpi-arm64.yml`; note that the newer system libraries may require a newer Raspberry Pi OS runtime.
- **Build fails while compiling a crate:** open the failed job, expand the `Build release executable` step, and copy the complete error log. The two variants compile independently, so check whether only desktop or KMS failed.
- **Executable reports a missing `.so` library on the Pi:** install the matching runtime package on Raspberry Pi OS. The GitHub runner supplies build-time development libraries, not every runtime library on the Pi.
- **KMS cannot open the display:** this is a hardware/runtime configuration issue, not a compile failure. Check that the Pi uses a DRM/KMS display driver and that the user has permission to access the relevant `/dev/dri` devices. Test locally on the Pi.
- **UI starts but shows no live data:** make sure the separate backend is running and the expected Unix socket exists.
