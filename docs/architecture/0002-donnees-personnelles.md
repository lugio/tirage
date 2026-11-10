# ADR 0002 — Données personnelles

- Statut : acceptée (03/11/2026)

## Contexte
« Qui imprime quoi » est une donnée personnelle des salariés. Le règlement général sur la protection des données et
les recommandations de la CNIL (journalisation ; surveillance des salariés) demandent : finalité définie,
minimisation, durée de conservation limitée, accès restreint, information préalable des personnes.

## Décision
- Champs collectés par travail : ceux du spouleur (utilisateur, poste, file, dates, pages, copies, feuilles, couleur,
  recto-verso, format, orientation, qualité, taille, statut). Le **nom du document** est un réglage :
  `non` | `application` (application d'origine seule, déduite du titre) | `complet`. Valeur par défaut : `application`.
- Conservation : détail par utilisateur **12 mois glissants** (réglable), puis seulement des totaux mensuels sans nom
  (par service, par copieur).
- Accès : connexion par annuaire, un groupe autorisé ; **chaque consultation du détail par utilisateur est journalisée**.
- Aucun envoi à un tiers ; aucune donnée personnelle dans les journaux techniques.
- L'outil fournit un modèle de note d'information des personnes ; la décision et le registre restent à l'organisation.

## Mise en œuvre de la conservation (04/11/2026)
- Passage quotidien, au démarrage puis toutes les 24 h, en une transaction : le détail d'avant la limite devient des
  totaux par mois, service, site, file, couleur, recto-verso et format ; il est effacé, ainsi que les fiches de
  l'annuaire devenues sans objet. Effacement réel dans le fichier (`secure_delete`, vidage du journal d'écriture).
- Le service et le site des totaux sont ceux connus au moment de la purge.
- **Limite** : un total « sans nom » reste rattachable à une personne si elle est seule dans son service ou sur son
  copieur. C'est un pseudonymat de fait, à garder en tête avant d'élargir les lecteurs.
- Journal des consultations : 12 mois par défaut (la CNIL recommande de 6 mois à 1 an pour une journalisation).
- Les sauvegardes contiennent les mêmes données : leur durée de conservation doit rester cohérente avec celle-ci.
