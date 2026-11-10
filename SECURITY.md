# Security policy / Sécurité

## Reporting a vulnerability

Please report it **privately**, through the
[Security tab](https://github.com/lugio/tirage/security/advisories/new) of this repository (private security
advisory) — never in a public issue. Expect a first answer within 7 days.

Signaler une faille **en privé**, par l'onglet « Security » du dépôt (avis de sécurité privé), jamais par un ticket
public. Réponse visée sous 7 jours.

## Supported versions

Only the latest release receives security fixes.

## Design principles

- No outbound connection to any third party: no telemetry, no automatic update.
- Collector → server: HTTPS, one token per collector, revocable; the server only stores its hash.
- Secrets (tokens, LDAP read account, SNMP community) live only in the local configuration, never in the repository or
  in logs.
- Web interface: server-rendered pages, strict Content-Security-Policy without inline script or style.
- Every view of an individual's detail is written to an audit log.
