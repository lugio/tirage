# Guide — brancher l'annuaire

Sans section `[annuaire]`, le serveur ne fait que recevoir les événements : aucune page n'est servie.

## Préparer
1. Un **compte de lecture** dédié, sans autre droit.
2. Son mot de passe dans un fichier : `install -m 600 -o tirage /dev/stdin /etc/tirage/annuaire.secret`, puis saisir le
   mot de passe suivi de Ctrl-D. Il n'apparaît jamais dans la configuration.
3. Un **groupe** contenant les personnes autorisées à consulter. Les groupes imbriqués comptent.
4. Le **certificat de l'autorité** qui a signé celui de l'annuaire, en PEM (`ca`), s'il n'est pas dans le magasin du système.

## Configurer
Voir `crates/tirage-serveur/serveur.exemple.toml`. Pour `attribut_service` et `attribut_cabinet`, vérifier dans
l'annuaire quel attribut porte réellement l'information, ou utiliser `dn:ou` (première unité d'organisation du DN).

## HTTPS
Le serveur écoute en HTTP sur la boucle locale, derrière un mandataire HTTPS **sur la même machine** : le mot de passe de
connexion passe par ce lien. Le cookie porte `Secure` : sans HTTPS, la connexion ne tient pas (voulu).
`http_en_clair = true` retire cet attribut, pour un essai local seulement.

## Vérifier
- Un compte hors du groupe est refusé, avec le même message qu'un mauvais mot de passe.
- Dans le journal du serveur : `annuaire : N fiche(s) relue(s), M absente(s)`. Aucun nom n'y figure. Si tout est absent,
  revoir `base`, `filtre_personnes` et `attribut_identifiant`.
- La page « Journal des consultations » trace chaque connexion et chaque détail consulté.

## Limites
- Annuaire Active Directory requis pour le contrôle du groupe (règle de groupes imbriqués).
- Le service affiché est le service actuel, y compris pour les travaux passés.
- Retrait du groupe : effet à la session suivante.
