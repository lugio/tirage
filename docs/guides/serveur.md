# Guide — poser le serveur

Sur Debian, avec nginx devant. Le serveur écoute en HTTP sur la boucle locale ; sans section `[annuaire]`, il ne fait que
recevoir les événements.

1. Binaire dans `/usr/local/bin/tirage-serveur`, compte système `tirage` sans connexion.
2. Configuration `/etc/tirage/serveur.toml` (`root:tirage`, 640), d'après `crates/tirage-serveur/serveur.exemple.toml`.
3. `deploiement/tirage-serveur.service` dans `/etc/systemd/system/`. La base va dans `/var/lib/tirage`, seul endroit
   où le service peut écrire.
4. `deploiement/nginx-tirage.conf.exemple`, à adapter : les listes `allow` limitent la réception au serveur d'impression
   et les pages aux postes des lecteurs. Clé du certificat en 600.
5. Jeton du collecteur : `tirage-serveur --nouveau-jeton NOM`, empreinte dans la configuration, jeton côté collecteur.

## Certificat
Une autorité interne, de préférence. Un certificat auto-signé convient à un essai, si le collecteur le désigne par `ca`.
Il doit alors être `CA:FALSE` avec le nom ou l'adresse en SAN : le client TLS du collecteur refuse un certificat
d'autorité présenté par un serveur.

## Dimensionnement
Mesures sur base fictive (titre de document de 75 caractères) : environ 500 octets par travail, moins de 15 Mo de mémoire
pour le serveur. 1,5 million de travaux par an : 750 Mo. Le fichier ne rétrécit pas après une purge : la place libérée
est réutilisée. La base est verrouillée pendant une requête : un rapport de 12 mois sur 1,5 million de travaux prend
quelques secondes, pendant lesquelles la réception attend.

## Données personnelles
- Le journal du service ne cite aucune personne. Celui de nginx, oui : `access_log off` et niveau `crit` sont voulus.
- Les sauvegardes de la machine contiennent la base : leur durée doit rester cohérente avec la conservation (ADR 0002).

## Réglages d'affichage (`[interface]`)

| Réglage | Par défaut | Rôle |
|---|---|---|
| `gros_travail_pages` | 100 | Un travail compte comme gros à partir de ce nombre de pages, copies comprises. |
| `collecte_silence_minutes` | 120 | Temps ouvré sans travail reçu au bout duquel la collecte est signalée comme muette. |
| `heure_ouverture`, `heure_fermeture` | 8, 18 | Heures ouvrées, du lundi au vendredi, dans le fuseau configuré. Les jours fériés ne sont pas connus : l'avertissement peut alors être faux. |

L'activité par heure et les gros travaux d'un copieur ne s'affichent qu'à partir de trois personnes sur la période : en dessous, ils décriraient une personne. Ils ne sont jamais restitués par personne ni par service.
