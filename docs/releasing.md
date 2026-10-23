# Releasing FastCapture

Every release follows these steps, in this order. The README and its images come before the
version bump because the release zip ships `README.md`.

## 1. Start from `main`

Everything going into the release is merged to `main`, CI is green, and you are on a clean `main`
in `D:\Projects\FastCapture` (the README image tool builds that checkout).

## 2. Bring the README up to date

List what changed since the last release:

```powershell
git log --oneline v<previous>..main
```

For every change a user would notice, update `README.md`: new or changed keys in the capture box
and review window tables, new settings in Settings and in the settings file table, new providers or
models, changed limits, costs or paths, and the FAQ when an answer changed. Keep the README's
voice: short, plain sentences that say what happens.

## 3. Regenerate the README images

In `D:\Projects\FastCapture-shots` (private, local; see its README), with nothing else in front of
the primary monitor:

```powershell
./tools/readme-images.ps1
```

It captures the real windows on staged notes, composes `hero.png` and `themes.png`, and replaces
only the images in `docs\images\` whose pixels changed. Then:

- Look at every changed image. The sample notes are dated the day of the run, so `review.png` and
  `hero.png` change at every release by their dates alone. That is expected.
- When a window changed shape, or the release adds a window or feature worth showing, add or adjust
  a scene in `src\scene.rs` and the `$plain` or composition map in `tools\readme-images.ps1`, then
  reference the new image from the README.
- An image the README no longer references is deleted.

## 4. Bump the version

The same `x.y.z` in:

- `Cargo.toml` (`version`), then `cargo build` to update `Cargo.lock`
- `assets/fastcapture.manifest` (`assemblyIdentity` `version="x.y.z.0"`)

## 5. Commit, merge, build

Commit the README, images and version bump as `FastCapture x.y.z` and merge it to `main`. CI's
`package` job on `main` builds `FastCapture-x.y.z-windows-x64.zip` and `-arm64.zip` with their
`.sha256` files as artifacts:

```powershell
gh run download <run id> -R coccor/FastCapture --dir dist
```

## 6. Publish the release

```powershell
$assets = (Get-ChildItem dist -Recurse -Include *.zip, *.sha256).FullName
gh release create vx.y.z -R coccor/FastCapture --title "FastCapture x.y.z" --notes-file notes.md @assets
```

The notes say what the release brings in a sentence, then one bullet per change, and end with:

> ## Download
>
> Portable zips for Windows 10 (1809 or later) and 11, x64 and Arm64. Unzip and run
> `fastcapture.exe`; the `.sha256` files hold each zip's checksum. See the README for setting up
> each model.
