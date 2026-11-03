# Sécurité

Signaler une faille **en privé**, par l'onglet « Security » du dépôt (avis de sécurité privé), jamais par un ticket
public. Réponse visée sous 7 jours.

Principes du projet :
- aucune connexion sortante vers un tiers (pas de télémétrie, pas de mise à jour automatique) ;
- collecteur → serveur : HTTPS, un jeton par collecteur, révocable ;
- secrets (jetons, compte de lecture LDAP, communauté SNMP) uniquement dans la configuration locale, jamais dans le
  dépôt ni dans les journaux ;
- consultation du détail par utilisateur journalisée.
