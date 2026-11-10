# Journal des versions

Format inspiré de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/), versions selon [SemVer](https://semver.org/lang/fr/).

## [Non publié]

## [0.2.1] - 2026-11-10

### Ajouté
- Licence commerciale possible sur demande, en plus de l'`AGPL-3.0-only` ; accord de contribution annoncé dans `CONTRIBUTING.md`.
- Interface : pied de page avec la version et le lien vers le code source (article 13 de l'AGPL).

### Modifié
- Dépendances mises à jour (Dependabot, PR #1).

## [0.2.0] - 2026-11-10
Première version avec archives prêtes à l'emploi.

### Ajouté
- Installation simple : archives prêtes à l'emploi publiées à chaque version (collecteur Windows, serveur Linux, empreintes), scripts `installer-serveur.sh` (crée tout, déclare le collecteur et donne son jeton ; relancé, met à jour sans toucher à la configuration) et `installer-collecteur.ps1` / `retirer-collecteur.ps1`, guide `docs/guides/installation.md`.

### Modifié
- Interface : logo dans l'en-tête et en icône d'onglet (`/logo.svg`) ; couleur de la marque passée du vert à l'ardoise, distincte des séries N&B et couleur des graphiques.

## [0.1.0] - 2026-11-10
Première version publiée.

### Modifié
- Licence AGPL-3.0-only ; README en anglais et en français avec captures sur données fictives et icône ; code de conduite, modèles de ticket et de PR, CODEOWNERS, Dependabot.
- Consommables : chaque jauge a la couleur de son toner, le pourcentage d'un toner bas ressort, titres de colonnes alignés sur les jauges.
- Interface, à la suite d'un audit : rubrique Rapports dans la navigation, classements cliquables sur la période choisie, journal en français, niveaux et toners bas repérables d'un coup d'œil, intitulés et noms longs qui passent à la ligne, barres de classement dans une teinte distincte des séries N&B et couleur, raccourcis de période sur le palmarès, retour au tableau de bord sur une page d'erreur, lien d'évitement et focus visibles au clavier.

### Ajouté
- Interface : résumé des copieurs sur l'accueil, tableau des consommables du toner le plus bas au plus haut, fiche d'un copieur avec son relevé, ses problèmes, ses consommables et ses pages produites dans le temps.
- Interface, page Copieurs : pages réellement produites sur la période d'après les compteurs, copies comprises ; bloc « À traiter » avec copieur muet en heures ouvrées et alertes traduites, classées par gravité, sans la veille.
- Collecteur : relevé automatique des copieurs déclarés (`[snmp] intervalle_minutes`, 60 par défaut), envoyé par la file ; profil Konica Minolta avec compteurs noir et couleur, copie et impression, y compris sur un appareil sans fonction copie.
- Interface : page Tarifs, prix par page N&B et couleur, général ou par copieur, gardés en base, inscrits au journal ; ils priment sur `[couts]`.
- Interface : état de la collecte (dernier travail reçu par collecteur, avertissement s'il est muet en heures ouvrées) ; activité par jour et par heure, globale ou par copieur, jamais par personne ; rapport par application ; gros travaux par copieur. Seuls les travaux en couleur sont signalés comme gros. Réglages `gros_travail_couleur_pages`, `collecte_silence_minutes`, `heure_ouverture`, `heure_fermeture`.
- Interface : page Copieurs alimentée par les travaux (activité, personnes, non imprimés, dernier travail), sans attendre le relevé SNMP ; raccourcis de période ; comparaison à la période précédente et feuilles évitées par le recto-verso ; vue d'un service, d'un cabinet ou d'un copieur, journalisée ; fiche d'une personne avec ses totaux, son évolution, ses copieurs, et durée, taille, résolution et orientation de chaque travail.
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
- Tableau de bord : évolution des pages par jour ou par mois (colonnes empilées, chiffres dépliables), répartition noir et blanc / couleur, classements. Graphiques en SVG générés côté serveur, sans script.
- Interface fluide et responsive, thème sombre selon le système. Style dans `style.css`, composants partagés dans `interface/gabarit.rs` et `interface/graphique.rs`.

### Corrigé
- Sécurité : SQLite embarqué mis à jour (rusqlite 0.41, libsqlite3-sys 0.39), qui corrige RUSTSEC-2026-0459 (lecture de mémoire non initialisée avec un fichier `-wal` fabriqué ; attaque locale).
- Connexion : l'appartenance au groupe se lit depuis le groupe (`member`, groupes imbriqués compris). Un compte de lecture sans droit sur `memberOf` faisait refuser tout le monde, sans erreur.
- La taille d'un travail ne retombe plus à 0 quand le spouleur le retire de la file.

[Non publié]: https://github.com/lugio/tirage/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/lugio/tirage/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/lugio/tirage/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/lugio/tirage/releases/tag/v0.1.0
