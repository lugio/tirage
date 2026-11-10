# Installer tirage

Trois étapes, une vingtaine de minutes. Il faut :

- un **serveur Linux** avec systemd (Debian 12, Ubuntu 22.04 ou plus récent), joignable en HTTPS depuis le serveur
  d'impression et depuis les postes des lecteurs ;
- le **serveur d'impression Windows** par lequel passent toutes les impressions, avec un accès administrateur ;
- pour l'interface : un **annuaire** LDAP ou Active Directory, un compte de lecture et un groupe des personnes autorisées.

Les archives se téléchargent sur la page [Releases](https://github.com/lugio/tirage/releases) ; `SHA256SUMS` donne
leurs empreintes, et les scripts revérifient celle du programme avant d'installer quoi que ce soit.

> Avant de collecter, informer les personnes concernées : [information des personnes](information-des-personnes.md).
> Les choix de conservation sont expliqués dans l'[ADR 0002](../architecture/0002-donnees-personnelles.md).

## 1. Serveur

```sh
tar -xzf tirage-serveur-X.Y.Z-linux-x86_64.tar.gz
cd tirage-serveur-X.Y.Z-linux-x86_64
sudo ./installer-serveur.sh
```

Le script crée le compte `tirage`, la configuration `/etc/tirage/serveur.toml`, la base dans `/var/lib/tirage` et le
service systemd, puis il **déclare un collecteur et affiche son jeton une seule fois** : le garder dans un coffre, il
sert à l'étape 2. Le serveur écoute alors sur `127.0.0.1:8080`.

**Publier en HTTPS** : adapter `nginx-tirage.conf.exemple` de l'archive (certificat, noms, listes `allow` : la
réception au seul serveur d'impression, les pages aux seuls postes des lecteurs). Un certificat d'une autorité
interne est le plus simple ; un certificat auto-signé convient s'il est `CA:FALSE` avec le nom en SAN
([détail](serveur.md#certificat)).

## 2. Collecteur

Sur le serveur d'impression, extraire `tirage-collecteur-X.Y.Z-windows-x86_64.zip`, puis dans un **PowerShell
administrateur**, depuis ce dossier :

```powershell
powershell -ExecutionPolicy Bypass -File .\installer-collecteur.ps1 -Serveur https://tirage.example.org -Nom impression-1
```

Coller le jeton de l'étape 1 quand il est demandé (rien ne s'affiche). Options : `-Autorite .\autorite.pem` si
Windows ne connaît pas l'autorité du certificat du serveur ; `-NomDocument non|application|complet` pour le titre des
documents gardé (`application` par défaut).

Imprimer une page : elle arrive sur le serveur dans la minute (`journalctl -u tirage-serveur` côté serveur,
`C:\ProgramData\tirage\donnees\journal\collecteur.log` côté Windows). Si le serveur est injoignable, les travaux
attendent sur disque et partent à son retour.

**Copieurs (facultatif)** : pour les compteurs N&B et couleur, les toners et les alertes, déclarer chaque copieur dans
`C:\ProgramData\tirage\collecteur.toml` (modèle : `collecteur.exemple.toml`), puis `Restart-Service tirage-collecteur`.
Le relevé est en SNMP, lecture seule, depuis le serveur d'impression (UDP 161 vers les copieurs).

## 3. Interface

Recopier la section `[annuaire]` de `serveur.exemple.toml` dans `/etc/tirage/serveur.toml` et l'adapter (adresse,
compte de lecture, groupe autorisé, attributs service et cabinet ; [guide de l'annuaire](annuaire.md)), mettre le mot
de passe du compte dans le fichier désigné (`chmod 600`, propriétaire `tirage`), puis :

```sh
sudo systemctl restart tirage-serveur
```

Se connecter avec un compte du groupe. Les prix par page se saisissent dans **Tarifs**.

## Mettre à jour

- **Serveur** : extraire la nouvelle archive et relancer `sudo ./installer-serveur.sh` : il remplace le programme et
  redémarre le service, sans toucher à la configuration ni à la base.
- **Collecteur** : `retirer-collecteur.ps1` (la configuration et la file restent), puis `installer-collecteur.ps1`
  depuis la nouvelle archive. Les travaux terminés pendant les quelques secondes d'arrêt ne sont pas suivis : choisir un
  moment calme.

Lire le [journal des versions](../../CHANGELOG.md) avant une mise à jour.

## Retirer

- **Collecteur** : `retirer-collecteur.ps1` ; avec `-ToutEffacer`, la configuration et la file partent aussi.
- **Serveur** : `systemctl disable --now tirage-serveur`, puis supprimer `/usr/local/bin/tirage-serveur`,
  `/etc/systemd/system/tirage-serveur.service`, `/etc/tirage` et `/var/lib/tirage` (la base contient des données
  personnelles : l'effacer selon votre politique de conservation).

## Aller plus loin

[Serveur en détail](serveur.md) (dimensionnement, réglages d'affichage, tarifs, sauvegarde) ·
[Annuaire](annuaire.md) · [Essai du collecteur en console et relevés SNMP](essai-collecteur.md) ·
[Construire depuis les sources](../../CONTRIBUTING.md#development)
