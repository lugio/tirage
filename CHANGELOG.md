# Journal des versions

## [Non publié]
### Ajouté
- Squelette de l'espace Cargo (modèle, file d'attente, collecteur, serveur), CI, politique de dépendances, ADR 0001-0002.
- Format d'échange v1 (`tirage-modele`) : travail d'impression, relevé de copieur, enveloppe versionnée, politique du nom de document (non / application / complet) et déduction de l'application d'origine.
- Collecteur, lecture du spouleur Windows (`tirage-collecteur --console`) : abonnement aux notifications de travaux du serveur d'impression local, réglages du pilote (DEVMODE : copies, couleur, recto-verso, format, orientation, résolution), suivi jusqu'à la sortie de file, dépôt dans la file d'attente. Guide d'essai : `docs/guides/essai-collecteur.md`.
- File d'attente sur disque (`tirage-file`) : écriture atomique, reprise après coupure, capacité bornée, mise de côté des éléments refusés.
