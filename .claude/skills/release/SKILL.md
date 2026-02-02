---
name: release
description: 'Tag release, update PKGBUILD checksums, and push to AUR.'
disable-model-invocation: true
allowed-tools: Read, Edit, Bash, Glob, Grep, AskUserQuestion
argument-hint: '[version]'
---

# release

Creates a git tag, updates PKGBUILD checksums, and pushes to AUR after PR merge.

## Critical Rules

- NEVER run on non-master/main branch without user confirmation
- NEVER force push tags
- ALWAYS verify tag doesn't already exist before creating
- ALWAYS use annotated tags for releases (not lightweight tags)

## Dependencies

- `tomlq` (or `dasel`) — TOML parsing for version extraction
- `pacman-contrib` — Provides `updpkgsums` for automatic checksum updates

## Workflow

### 1. Verify Environment

```bash
git branch --show-current
git remote show origin | grep 'HEAD branch'
```

If not on master/main → Use AskUserQuestion to confirm proceeding.

### 2. Determine Version

**$ARGUMENTS** — If provided, use as version (e.g., `1.1.1` or `v1.1.1`).

If not provided, extract from workspace `Cargo.toml`:

```bash
grep -A1 '^\[workspace\.package\]' Cargo.toml | grep 'version' | sed 's/.*"\(.*\)".*/\1/'
```

Normalize version: ensure tag format is `vX.Y.Z` (add `v` prefix if missing).

### 3. Update PKGBUILD Version

Update `pkgver` in PKGBUILD to match the release version:

```bash
sed -i "s/^pkgver=.*/pkgver=X.Y.Z/" PKGBUILD
```

Reset `pkgrel` to 1 if version changed:

```bash
sed -i "s/^pkgrel=.*/pkgrel=1/" PKGBUILD
```

### 4. Check Tag Status

```bash
git tag -l "vX.Y.Z"
git ls-remote --tags origin | grep "vX.Y.Z"
```

- Tag exists locally or remotely → Error: "Tag vX.Y.Z already exists."
- No tag → Proceed

### 5. Create and Push Tag

Use annotated tag with release message:

```bash
git pull origin main
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin vX.Y.Z
```

Output: `Created and pushed tag: vX.Y.Z`

### 6. Update Checksum

Wait briefly for GitHub to generate the tarball, then use `updpkgsums` to automatically update PKGBUILD checksums:

```bash
sleep 5  # Wait for GitHub to process the tag
updpkgsums
```

If checksum generation fails → Use AskUserQuestion: "Checksum generation failed. Retry or skip?"

### 7. Regenerate .SRCINFO

```bash
makepkg --printsrcinfo > .SRCINFO
```

### 8. Commit Checksum Update

```bash
git add PKGBUILD .SRCINFO
git commit -m "$(cat <<'EOF'
chore: update sha256sums for vX.Y.Z
EOF
)"
git push origin main
```

### 9. Push to AUR

Check if AUR remote exists:

```bash
git remote get-url aur
```

If AUR remote doesn't exist → Use AskUserQuestion: "AUR remote not configured. Add it now? (ssh://aur@aur.archlinux.org/mpvpaper-rs.git)"

If user confirms, add the remote:

```bash
git remote add aur ssh://aur@aur.archlinux.org/mpvpaper-rs.git
```

Push to AUR:

```bash
git push aur main
```

### 10. Output

```
Release vX.Y.Z completed!

Tag: vX.Y.Z (pushed to origin)
Checksum: updated
AUR: pushed

GitHub: https://github.com/bityoungjae/mpvpaper-rs/releases/tag/vX.Y.Z
AUR: https://aur.archlinux.org/packages/mpvpaper-rs
```

## Error Handling

| Situation                 | Action                                                                       |
| ------------------------- | ---------------------------------------------------------------------------- |
| Not on master/main        | Ask user to confirm or abort                                                 |
| Tag already exists        | Error and abort                                                              |
| updpkgsums not installed  | Error: "updpkgsums is required. Install with: sudo pacman -S pacman-contrib" |
| Checksum generation fails | Retry or skip with user confirmation                                         |
| Push fails                | Show error, suggest manual resolution                                        |
| AUR remote not configured | Ask user to add it or skip AUR push                                          |
| AUR push fails            | Show SSH key setup instructions if auth error                                |

## Rust-Specific Notes

- Version is extracted from workspace Cargo.toml (`[workspace.package].version`)
- Two binaries are installed: `mpvpaper-rs` and `mpvpaper-rs-holder`
- PKGBUILD uses `cargo build --frozen --release --all-features`
