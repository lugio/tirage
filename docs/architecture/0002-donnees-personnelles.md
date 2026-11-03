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
