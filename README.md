# tirage

Suivi des impressions d'un parc, **100 % auto-hébergé** : aucune donnée, mise à jour ou journal ne part chez un tiers.

- **Collecteur** (service Windows, sur le serveur d'impression) : lit chaque travail auprès du spouleur (utilisateur,
  poste, file, pages, copies, feuilles, couleur ou N&B selon le réglage du travail, recto-verso, format, statut) et
  relève les copieurs en SNMP (compteurs N&B / couleur, consommables, état). Les événements sont gardés sur disque
  jusqu'à l'accusé de réception du serveur.
- **Serveur** (Linux ou Windows) : stockage SQLite, tableaux par copieur / utilisateur / service, coûts, palmarès,
  état des copieurs, export CSV ; connexion par annuaire LDAP, accès limité à un groupe.

> État : **squelette (lot 0)**. Rien n'est encore utilisable.

## Construire

```sh
cargo build --workspace
cargo test --workspace
```

Chaîne Rust : voir `rust-toolchain.toml`. Le collecteur cible Windows ; le serveur, Linux et Windows.

## Documentation

- [Architecture](docs/architecture/0001-architecture.md)
- [Données personnelles](docs/architecture/0002-donnees-personnelles.md)
- [Contribuer](CONTRIBUTING.md) · [Sécurité](SECURITY.md) · [Journal des versions](CHANGELOG.md)

## Licence

Privé jusqu'à la v1 — voir [LICENSE](LICENSE).
