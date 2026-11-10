<p align="center">
  <img src="docs/images/icone.svg" width="96" height="96" alt="">
</p>

<h1 align="center">tirage</h1>

<p align="center">
  <b>Suivi des impressions auto-hébergé, pour un serveur d'impression Windows et ses copieurs.</b><br>
  Qui imprime quoi, où, en couleur ou en N&amp;B, pour quel coût, et combien de toner il reste.<br>
  Rien ne sort du réseau : ni nuage, ni télémétrie, ni mise à jour automatique.
</p>

<p align="center">
  <a href="LICENSE"><img alt="Licence : AGPL-3.0-only" src="https://img.shields.io/badge/licence-AGPL--3.0--only-0f5e58"></a>
  <img alt="Rust 1.99" src="https://img.shields.io/badge/rust-1.99-0f5e58">
  <img alt="Collecteur : Windows" src="https://img.shields.io/badge/collecteur-Windows-0f5e58"> <img alt="Serveur : Linux ou Windows" src="https://img.shields.io/badge/serveur-Linux%20%7C%20Windows-0f5e58">
</p>

<p align="center"><a href="README.md">English version</a></p>

<p align="center">
  <img src="docs/images/accueil.png" width="800" alt="Tableau de bord : pages imprimées, part de couleur, recto-verso, coût, activité par jour et par heure, classement des services et des copieurs (données fictives)">
</p>

> Toutes les captures utilisent des données fictives.

## Fonctions

- **Chaque travail, lu au spouleur** : utilisateur, poste, file, pages, copies, feuilles, couleur ou N&amp;B demandé au
  pilote, recto-verso, format, statut, application. Les travaux attendent sur disque l'accusé de réception du serveur.
- **Copieurs relevés en SNMP** : compteurs N&amp;B et couleur, toners et autres consommables, alertes traduites et
  classées ; pages réellement produites entre deux relevés, copies comprises. Printer-MIB (RFC 3805) et profils de
  marque en fichiers TOML, ajoutés sans recompiler : [ajouter sa marque](docs/guides/printer-profiles.md).
- **Tableaux** : par utilisateur, service, site, copieur, format, application ; activité par jour et par heure ;
  comparaison de périodes ; feuilles évitées par le recto-verso ; palmarès ; export CSV.
- **Coûts** : prix par page N&amp;B et couleur, général ou par copieur, saisi dans l'interface.
- **Connexion par l'annuaire** : LDAP ou Active Directory, accès limité à un groupe ; chaque consultation du détail
  d'une personne est inscrite au journal.
- **Protection des données dès la conception** : conservation réglable (titre des documents absent, application seule
  ou complet), purge automatique qui garde des totaux anonymes, guide d'information des personnes.

<table>
  <tr>
    <td><img src="docs/images/copieurs-sombre.png" alt="Page Copieurs en mode sombre : à traiter, jauges aux couleurs des toners, pages produites (données fictives)"></td>
    <td><img src="docs/images/fiche-copieur.png" alt="Fiche d'un copieur : relevé, consommables, pages produites dans le temps, travaux par personne (données fictives)"></td>
  </tr>
  <tr>
    <td align="center">Copieurs, mode sombre</td>
    <td align="center">Fiche d'un copieur</td>
  </tr>
</table>

## Fonctionnement

```
 Serveur d'impression Windows                 Serveur Linux (ou Windows)
┌───────────────────────────┐  HTTPS + jeton   ┌──────────────────────────────┐
│ tirage-collecteur         │ ───────────────► │ tirage-serveur               │
│  · notifications spouleur │  événements JSON │  · SQLite                    │
│  · relevés SNMP           │                  │  · pages rendues au serveur, │
│  · file sur disque        │                  │    graphiques SVG, sans JS   │
└───────────────────────────┘                  │  · connexion LDAP            │
        │ SNMP                                 └──────────────────────────────┘
        ▼                                                 ▲ HTTPS (nginx)
     copieurs                                             │ lecteurs
```

Deux binaires autonomes, sans dépendance à l'exécution. Détail : [ADR 0001](docs/architecture/0001-architecture.md).

## Démarrer

Télécharger les archives sur la page [Releases](https://github.com/lugio/tirage/releases), puis :

```sh
# 1. Serveur (Linux, systemd) : crée tout et affiche une fois le jeton du collecteur
tar -xzf tirage-serveur-X.Y.Z-linux-x86_64.tar.gz && cd tirage-serveur-X.Y.Z-linux-x86_64
sudo ./installer-serveur.sh
```

```powershell
# 2. Collecteur : sur le serveur d'impression Windows, PowerShell administrateur ; coller le jeton quand il est demandé
powershell -ExecutionPolicy Bypass -File .\installer-collecteur.ps1 -Serveur https://tirage.example.org
```

3. **Interface** : ajouter la section `[annuaire]` (LDAP ou Active Directory) à `/etc/tirage/serveur.toml` et redémarrer.

Guide pas à pas, HTTPS, copieurs, mises à jour et retrait : **[docs/guides/installation.md](docs/guides/installation.md)**.
Informer les personnes concernées avant la collecte :
[docs/guides/information-des-personnes.md](docs/guides/information-des-personnes.md).

Depuis les sources : `cargo build --release --workspace` (chaîne fixée dans `rust-toolchain.toml`).

## Organisation du dépôt

| Chemin | Contenu |
|---|---|
| `crates/tirage-modele` | Format d'échange versionné (travaux, relevés) |
| `crates/tirage-file` | File d'attente sur disque |
| `crates/tirage-snmp` | Client SNMP (lecture seule) |
| `crates/tirage-collecteur` | Service Windows : spouleur et SNMP ; profils de marque dans `profils/` |
| `crates/tirage-serveur` | Réception, stockage, interface web |
| `docs/architecture` | Décisions d'architecture |
| `docs/guides` | Guides d'installation et d'exploitation |

## Documentation

- Décisions : [architecture](docs/architecture/0001-architecture.md) ·
  [données personnelles](docs/architecture/0002-donnees-personnelles.md) ·
  [client SNMP](docs/architecture/0003-client-snmp.md) ·
  [annuaire et connexion](docs/architecture/0004-annuaire-et-connexion.md)
- [Journal des versions](CHANGELOG.md) · [Contribuer](CONTRIBUTING.md) · [Sécurité](SECURITY.md) ·
  [Code de conduite](CODE_OF_CONDUCT.md)

## Contribuer

Tickets et demandes de fusion bienvenus : voir [CONTRIBUTING.md](CONTRIBUTING.md). Le plus simple pour aider : un
[profil pour la marque de son copieur](docs/guides/printer-profiles.md), un fichier texte vérifié par la CI. Une faille se signale en privé,
comme décrit dans [SECURITY.md](SECURITY.md).

## Licence

Copyright © 2026 lugio. tirage est un logiciel libre sous [GNU Affero General Public License v3.0 seule](LICENSE)
(`AGPL-3.0-only`) : vous pouvez l'utiliser, l'étudier, le modifier et le partager. Si vous le modifiez et le faites
utiliser par d'autres à travers un réseau, vous devez leur proposer votre code source modifié — chaque page de
l'interface renvoie à son code source dans ce but.

Une **licence commerciale** (sans les obligations de l'AGPL) est possible sur demande : ouvrir un ticket intitulé
« Licence commerciale ».
