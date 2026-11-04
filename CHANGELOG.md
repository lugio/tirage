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
- Service Windows du collecteur (`--installer`, `--desinstaller`, `--service`) : LocalSystem, démarrage automatique différé après le spouleur, redémarrage après échec, arrêt propre (le fil d'envoi termine). Journal de fonctionnement horodaté sans aucune donnée de travail, taille bornée.
- Annuaire en lecture seule (`[annuaire]`, LDAPS) : service et cabinet des seules personnes qui ont imprimé, par recherche nominative, relus régulièrement ; dernières valeurs gardées pour un compte disparu. ADR 0004, guide `docs/guides/annuaire.md`.
- Connexion à l'interface réservée à un groupe (groupes imbriqués compris) : mot de passe vide refusé avant tout échange, refus indiscernables, limite d'essais par identifiant, session en mémoire (cookie `HttpOnly`, `Secure`, `SameSite=Strict`, seule l'empreinte du jeton est gardée).
- Journal des consultations : connexions, détail par utilisateur et lecture du journal, écrits avant de servir les données (journal indisponible = page non servie) ; lignes non modifiables.
- Interface : tableau de bord sur une période, rapports par utilisateur, service, cabinet, copieur et format (N&B, couleur, recto-verso, feuilles, coût), palmarès, état des copieurs, détail par utilisateur, dates dans le fuseau configuré. Chaque lecture est au journal.
- Coûts (`[couts]`) : prix par page N&B et couleur, général ou par file ; une file sans tarif est signalée, un mode couleur inconnu est chiffré au tarif noir.
- Export CSV pour tableur (UTF-8 avec BOM, point-virgule, valeurs prises pour une formule neutralisées), sans nom de document.
- Conservation (`[conservation]`, 12 mois par défaut) : le détail passe en totaux mensuels sans nom, effacement réel dans le fichier, fiches sans objet retirées, journal des consultations purgé ; les rapports continuent de compter ces totaux pour les mois entiers.
- Sauvegarde cohérente de la base (`--config F --sauvegarder COPIE`), serveur en marche, fichier neuf en droits 600, sans écrasement.
- Modèle de note d'information des personnes (`docs/guides/information-des-personnes.md`).
- Pose du serveur : unité systemd durcie et exemple nginx (`deploiement/`), guide avec certificat, dimensionnement et données personnelles (`docs/guides/serveur.md`).

### Corrigé
- La taille d'un travail ne retombe plus à 0 quand le spouleur le retire de la file.
