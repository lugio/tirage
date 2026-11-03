# Journal des versions

## [Non publié]
### Ajouté
- Squelette de l'espace Cargo (modèle, file d'attente, collecteur, serveur), CI, politique de dépendances, ADR 0001-0002.
- Format d'échange v1 (`tirage-modele`) : travail d'impression, relevé de copieur, enveloppe versionnée, politique du nom de document (non / application / complet) et déduction de l'application d'origine.
- Collecteur, lecture du spouleur Windows (`tirage-collecteur --console`) : abonnement aux notifications de travaux du serveur d'impression local, réglages du pilote (DEVMODE : copies, couleur, recto-verso, format, orientation, résolution), suivi jusqu'à la sortie de file, dépôt dans la file d'attente. Guide d'essai : `docs/guides/essai-collecteur.md`.
- File d'attente sur disque (`tirage-file`) : écriture atomique, reprise après coupure, capacité bornée, mise de côté des éléments refusés.
- Client SNMP v2c en lecture seule (`tirage-snmp`), sans dépendance : codage BER, GET, GETNEXT, parcours de sous-arbre, faux agent pour les essais. ADR 0003.
- Relevé des copieurs (`tirage-collecteur --releve`) : identité, compteurs selon un profil de marque en fichier TOML (repli Printer-MIB), consommables, état et alertes (Host-Resources, Printer-MIB) ; `--parcourir` pour écrire un profil ; configuration du collecteur (`collecteur.exemple.toml`).
- Serveur de réception (`tirage-serveur --config`) : `POST /api/v1/evenements`, jeton par collecteur (seule son empreinte est configurée ; `--nouveau-jeton`), base SQLite versionnée, idempotence (un travail renvoyé n'est jamais compté deux fois), refus par élément sans bloquer le lot.
- Envoi par le collecteur (`--console --config`) : fil dédié qui vide la file vers le serveur en HTTPS (magasin de certificats du système ou autorité fournie ; `http://` refusé sauf essai), acquittés retirés, refusés mis de côté, délai croissant si le serveur est injoignable. Essai de bout en bout collecteur → serveur.

### Corrigé
- La taille d'un travail ne retombe plus à 0 quand le spouleur le retire de la file.
