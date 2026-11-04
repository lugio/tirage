# ADR 0004 — Annuaire, connexion et journal des consultations

- Statut : acceptée (04/11/2026)

## Contexte
Les restitutions se font par service et par cabinet, et le détail par utilisateur est réservé à un groupe. Tout passe
par l'annuaire de l'organisation, en lecture seule. Qui imprime quoi étant une donnée personnelle (ADR 0002), l'outil
lit le minimum et garde la trace de ceux qui consultent.

## Décision
- **Client** : crate `ldap3` avec `rustls` (licences compatibles, pas d'OpenSSL). `ldaps://` obligatoire ; `ldap://`
  exige `ldap_en_clair = true`, réservé aux essais.
- **Lecture nominative** : seules les personnes vues dans les travaux sont cherchées, par identifiant exact et par
  paquets. L'annuaire n'est jamais recopié. Un compte disparu garde ses dernières valeurs pour l'historique.
- **Rattachement** : attributs configurables, ou première unité d'organisation du DN (`dn:ou`). Le service affiché est
  le service **actuel** de la personne : un changement de service s'applique aussi à ses travaux passés.
- **Connexion** : recherche sous le compte de lecture avec le groupe dans le filtre (règle
  `LDAP_MATCHING_RULE_IN_CHAIN`, propre à Active Directory, pour les groupes imbriqués), puis liaison avec le mot de
  passe sur une connexion à part. Hors groupe, le mot de passe n'est pas essayé : aucun échec n'est compté sur le compte.
- **Mot de passe vide refusé avant tout échange** : en LDAP, il donnerait une liaison anonyme « réussie ».
- **Sessions en mémoire** : un redémarrage demande de se reconnecter ; seule l'empreinte du jeton est gardée. Le groupe
  est vérifié à la connexion : un retrait du groupe s'applique à la session suivante (4 h au plus par défaut).
- **Limite d'essais** par identifiant (5 en 15 minutes), pour ne pas verrouiller le compte dans l'annuaire. Revers
  accepté : un tiers peut bloquer un identifiant dans l'outil pendant 15 minutes, sans effet sur le compte lui-même.
- **Journal avant les données** : la consultation est écrite d'abord ; si le journal est indisponible, rien n'est servi.
  Un essai refusé dont l'identifiant n'est pas connu est consigné sans ce qui a été saisi (on tape parfois son mot de
  passe dans le mauvais champ). Le journal est non modifiable par l'outil ; sa durée de conservation relève du lot
  « conservation ». Quiconque accède au fichier de la base peut l'altérer : il reste une trace d'usage, pas une preuve.
- **Pas de mode sans authentification** : sans `[annuaire]`, l'interface n'existe pas (404), seule la réception tourne.

## Conséquences
- Toute route de données s'ajoute au routeur `protegees`, où la session est exigée par une couche unique.
- Le serveur parle HTTP : le mot de passe transite en clair entre le mandataire HTTPS et lui. Le mandataire doit donc
  être sur la même machine, sur la boucle locale.
