# Essai du collecteur en mode console

Le mode console lit le spouleur **local** de la machine où il tourne : on peut donc l'essayer sur n'importe quel
poste Windows 10/11 ou serveur, avec une imprimante virtuelle (« Microsoft Print to PDF »), sans toucher à un serveur
d'impression.

## Construire (depuis Linux)

```sh
cargo install cargo-zigbuild   # nécessite zig dans le PATH
cargo zigbuild --release -p tirage-collecteur --target x86_64-pc-windows-gnu
# → target/x86_64-pc-windows-gnu/release/tirage-collecteur.exe
```

Sous Windows : `cargo build --release -p tirage-collecteur`.

## Lancer

```powershell
.\tirage-collecteur.exe --console --donnees C:\essai-tirage --collecteur essai --nom-document application
```

Puis imprimer un document. Quand le travail quitte la file, une ligne JSON s'affiche (utilisateur, poste, file,
pages, copies, couleur, recto-verso, format…) et un fichier `.evt` apparaît dans `C:\essai-tirage\a-envoyer`.
Ctrl+C pour arrêter ; supprimer le dossier d'essai ensuite.

## À observer

- Le mode couleur est celui **demandé au pilote** ; vérifier avec un travail N&B puis couleur.
- Recto-verso et nombre de copies : selon ce que le pilote place dans la DEVMODE.
- Pages : certains pilotes ne donnent le total qu'en fin de travail.
