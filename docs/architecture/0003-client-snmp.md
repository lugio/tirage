# 0003 — Client SNMP écrit pour l'outil, en lecture seule

- Statut : accepté
- Date : 2026-11-03

## Contexte

Le collecteur relève les compteurs, consommables et états des copieurs par SNMP. Il lui faut GET et GETNEXT en v2c ;
SNMP v3 seulement si un appareil l'impose. Les bibliothèques Rust existantes tirent soit un environnement asynchrone
complet, soit une bibliothèque cryptographique en C (gênante pour construire l'exécutable Windows depuis Linux), soit
ne sont plus entretenues.

## Décision

Un crate `tirage-snmp` sans dépendance : codage BER du sous-ensemble utile (RFC 3416), client bloquant sur UDP avec
délai et essais, parcours borné d'un sous-arbre, faux agent en mémoire pour les essais.

- **Lecture seule par construction** : seuls les types de PDU GET, GETNEXT et Réponse existent dans le code ; aucun
  SET ne peut être émis.
- Un parcours s'arrête à la sortie du sous-arbre, à la fin de la MIB, à une borne de lignes, ou si l'appareil ne
  progresse plus (OID non croissant).
- Les requêtes sont découpées par paquets de 10 OID.

## Conséquences

- Environ 600 lignes à maintenir, couvertes par des essais (dont un message encodé à l'octet près et un échange UDP réel).
- SNMP v3 (authentification, chiffrement) demandera un ajout : il se fera derrière le même `Client` si un appareil
  du parc l'exige.
