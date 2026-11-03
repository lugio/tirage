# ADR 0001 — Architecture : collecteur + serveur, tout auto-hébergé

- Statut : acceptée (03/11/2026)

## Contexte
Il faut suivre les impressions d'un parc dont **toutes** les impressions passent par un serveur d'impression Windows,
et les restituer (par copieur, utilisateur, service ; N&B / couleur ; recto-verso ; format ; coûts ; palmarès ; état
des copieurs). Les produits du marché de ce type envoient souvent les données à un service en ligne ; ici, rien ne doit
sortir.

## Décision
1. **Collecteur** sur le serveur d'impression, service Windows :
   - travaux lus par l'API du spouleur (notification de changement de file, attributs du travail et réglages du
     pilote — DEVMODE). La couleur par utilisateur est celle **demandée au pilote** ; le contenu des travaux n'est pas
     analysé ;
   - copieurs relevés en SNMP : Printer-MIB standard (RFC 3805) + **profils de marque en fichiers** (OID des compteurs
     N&B / couleur), ajoutables sans recompiler ; les copieurs sont déclarés, pas découverts par balayage ;
   - file d'attente sur disque : un événement n'est retiré qu'après accusé de réception.
2. **Serveur** séparé (Linux de préférence) : `axum`, SQLite, pages rendues côté serveur, graphiques SVG ; un seul
   binaire.
3. Format d'échange JSON versionné (`tirage-modele`).

## Écartées
- Tout sur le serveur d'impression : appli web sur un serveur d'infrastructure, sauvegarde à part.
- Journal Windows `PrintService/Operational` (événement 307) via un SIEM : ni couleur ni recto-verso.
- Analyse du contenu du spool (pages couleur réelles) : coûteux (un analyseur par langage d'impression et par pilote) ;
  les compteurs SNMP donnent déjà le vrai N&B / couleur par copieur.

## Conséquences
- Un flux réseau collecteur → serveur à ouvrir ; le SNMP part du serveur d'impression.
- Le chiffre « couleur » par utilisateur peut surestimer (page sans couleur d'un travail couleur) : affiché comme tel.
