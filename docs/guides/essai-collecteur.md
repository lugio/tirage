# Essai du collecteur en mode console

Le mode console lit le spouleur **local** de la machine où il tourne : on peut l'essayer sur un poste Windows 10/11
avec une imprimante virtuelle (« Microsoft Print to PDF »), ou sur le serveur d'impression lui-même.

> Une imprimante **partagée** (`\\serveur\file`) vue d'un poste est une *connexion* : ses travaux vont directement au
> spouleur du serveur et n'apparaissent pas dans celui du poste. Pour voir les vrais travaux et les vrais pilotes des
> copieurs, le collecteur tourne **sur le serveur d'impression** (PowerShell administrateur).

## Construire (depuis Linux)

```sh
cargo install cargo-zigbuild   # nécessite zig dans le PATH
cargo zigbuild --release -p tirage-collecteur --target x86_64-pc-windows-gnu
# → target/x86_64-pc-windows-gnu/release/tirage-collecteur.exe
```

Sous Windows : `cargo build --release -p tirage-collecteur`.

Lint pour Windows depuis Linux (les dépendances en C se compilent avec zig) : poser `CC_x86_64_pc_windows_gnu` et
`AR_x86_64_pc_windows_gnu` sur les enveloppes créées par `cargo zigbuild` (`~/.cache/cargo-zigbuild/…/wrappers/`), puis
`cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu`.

## Envoi au serveur

Avec `--config` et une section `[serveur]` (url, jeton), le mode console envoie la file au serveur en continu.
Le jeton se fabrique côté serveur : `tirage-serveur --nouveau-jeton impression-1` affiche le jeton (à mettre dans la
configuration du collecteur) et la ligne `[[collecteurs]]` (empreinte seule) pour celle du serveur.

## Lancer

```powershell
.\tirage-collecteur.exe --console --donnees C:\essai-tirage --collecteur essai --nom-document application
```

Puis imprimer un document. Quand le travail quitte la file, une ligne JSON s'affiche (utilisateur, poste, file,
pages, copies, couleur, recto-verso, format…) et un fichier `.evt` apparaît dans `C:\essai-tirage\a-envoyer`.
Ctrl+C pour arrêter ; supprimer le dossier d'essai ensuite.

## À observer

- Le mode couleur est celui **demandé au pilote** (propriétés de l'imprimante) ; vérifier avec un travail N&B puis
  couleur. Un N&B demandé **seulement dans le logiciel** (« niveaux de gris » d'un lecteur PDF ou d'un navigateur)
  ne change pas le réglage du pilote : le travail reste annoncé en couleur. Les compteurs SNMP de l'appareil donnent
  le vrai partage N&B / couleur.
- Recto-verso et nombre de copies : selon ce que le pilote place dans la DEVMODE. Un pilote virtuel (PDF) ne gère
  pas le recto-verso : « inconnu » est attendu.
- Pages : certains pilotes ne donnent le total qu'en fin de travail ; le collecteur garde la plus grande valeur vue
  (pages comme taille, que le spouleur remet à 0 en sortie de file).

## Service Windows

Sur le serveur d'impression, en PowerShell **administrateur** :

```powershell
# Dossier du collecteur, configuration lisible par les seuls administrateurs et SYSTEM (elle contient le jeton)
New-Item -ItemType Directory C:\ProgramData\tirage -Force
icacls C:\ProgramData\tirage\collecteur.toml /inheritance:r /grant:r "SYSTEM:(R)" "Administrateurs:(F)"

.\tirage-collecteur.exe --installer --config C:\ProgramData\tirage\collecteur.toml
Start-Service tirage-collecteur
Get-Content C:\ProgramData\tirage\donnees\journal\collecteur.log -Tail 20
```

- Compte **LocalSystem**, démarrage **automatique différé**, dépend du spouleur ; redémarrage automatique 60 s après
  un échec (trois fois, compteur remis à zéro chaque jour).
- Le journal (`<donnees>\journal\collecteur.log`, 5 Mo puis `.log.1`) ne contient **aucun travail** : démarrage,
  arrêt, erreurs, état de l'envoi.
- Retour arrière : `.\tirage-collecteur.exe --desinstaller` (arrête et supprime le service ; données et configuration
  restent en place, à effacer à la main si voulu).

## Relevé SNMP d'un copieur (lecture seule)

Les copieurs se déclarent dans un fichier de configuration (modèle : `crates/tirage-collecteur/collecteur.exemple.toml`).
Ce fichier contient la communauté SNMP : il reste sur la machine du collecteur, lisible par son seul compte, jamais
dans un dépôt. La communauté n'est jamais passée en ligne de commande ni affichée.

```powershell
# Un relevé de chaque copieur (identité, compteurs, consommables, état, alertes), en JSON
.\tirage-collecteur.exe --releve --config C:\essai-tirage\collecteur.toml

# Valeurs d'un sous-arbre d'un appareil, pour écrire le profil d'une marque (ici la MIB privée d'un numéro d'entreprise)
.\tirage-collecteur.exe --parcourir 1.3.6.1.4.1.<entreprise> --copieur COPIEUR-EXEMPLE-1 --config C:\essai-tirage\collecteur.toml > parcours.txt
```

Le client SNMP ne sait que lire (GET, GETNEXT) : aucune requête d'écriture n'existe dans le code. La sortie d'un
parcours contient l'identité de l'appareil (numéro de série…) : la traiter comme une donnée interne.

### Profils de marque

`crates/tirage-collecteur/profils/*.toml` (intégrés à l'exécutable) ; d'autres profils peuvent être déposés dans
`[snmp] dossier_profils`. Un profil donne les OID à **additionner** pour les compteurs total, noir et couleur ; il
est choisi d'après l'identifiant de l'appareil (`sysObjectID` sous `1.3.6.1.4.1.<entreprise>`), à défaut la
Printer-MIB standard (total seul). Un compteur dont un seul OID manque n'est pas publié (jamais de total partiel).
