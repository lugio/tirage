# Contributing / Contribuer

Thank you for your interest! Issues and pull requests are welcome, in English or in French.

## Before you start

- For anything larger than a small fix, open an issue first so we can agree on the approach.
- Read the [architecture decisions](docs/architecture/) — especially
  [personal data](docs/architecture/0002-donnees-personnelles.md): this tool describes people at work.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check          # licences and advisories
```

The Rust toolchain is pinned in `rust-toolchain.toml`. The collector targets Windows; CI runs tests on Linux and
Windows.

## Rules

- **One branch and one pull request per change**; CI must be green (format, clippy without warnings, tests on Linux and
  Windows, `cargo-deny`, secret scan) before merging.
- **Commit messages** follow `type(scope): summary` (`feat`, `fix`, `docs`, `chore`, `test`, `refactor`). Existing
  history is in French; either language is fine.
- **No real data, ever** — no organisation, person, host or document name, internal address or log export. Examples
  and fixtures are **fictitious**: `example.org`, `192.0.2.0/24`, `EXEMPLE`, made-up printers and users. Screenshots
  are taken on fictitious data only.
- **Generic code** — nothing specific to one site in the code or in default values: configuration does it.
- **Dependencies** — permissive licences only (see `deny.toml`).
- **User-visible change** — add a line to `CHANGELOG.md` under « Non publié ».

## Releasing (maintainers)

1. In one pull request: bump `version` in `Cargo.toml` (SemVer), rename « Non publié » in `CHANGELOG.md` to
   `[X.Y.Z] - YYYY-MM-DD` and add a fresh « Non publié » section above it.
2. After merging: `git tag -a vX.Y.Z -m "tirage X.Y.Z"` on `main`, then `git push origin vX.Y.Z`.
3. The [Version workflow](.github/workflows/version.yml) checks that the tag matches `Cargo.toml` and the changelog,
   runs the tests, builds the Windows collector and the Linux server archives, and publishes the release with
   `SHA256SUMS` and the changelog section as notes.

## Résumé en français

Une branche et une PR par changement, CI verte avant fusion, messages `type(portée): résumé`. **Aucune donnée réelle** :
exemples, jeux d'essai et captures fictifs. Rien de propre à un site dans le code ni les valeurs par défaut.
Dépendances aux licences permissives. Toute évolution visible va dans le `CHANGELOG.md`. Version : PR qui monte `Cargo.toml` et date le `CHANGELOG.md`, puis tag `vX.Y.Z` : la CI publie les archives.
