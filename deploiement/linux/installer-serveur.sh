#!/bin/sh
# Installe ou met à jour le serveur tirage sur Linux (systemd). À lancer en root, depuis le dossier de l'archive.
#
#   sudo ./installer-serveur.sh            # première installation : crée tout et donne le jeton du collecteur
#   sudo ./installer-serveur.sh            # avec une archive plus récente : met à jour le programme, garde tout le reste
#
# Première installation : compte système « tirage », /etc/tirage/serveur.toml (640 root:tirage), base dans
# /var/lib/tirage (700), service systemd démarré, un collecteur déclaré et son jeton affiché UNE fois.
# Rien n'est écrit si un contrôle échoue. La configuration existante n'est jamais modifiée.
set -eu

ici=$(cd "$(dirname "$0")" && pwd)
bin=/usr/local/bin/tirage-serveur
conf=/etc/tirage/serveur.toml
unite=/etc/systemd/system/tirage-serveur.service

echec() { echo "erreur : $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || echec "lancer en root (sudo)."
command -v systemctl >/dev/null || echec "systemd introuvable : voir docs/guides/serveur.md pour une pose à la main."
[ -x "$ici/tirage-serveur" ] || echec "programme introuvable : $ici/tirage-serveur"

echo "1. Empreinte du programme"
if [ -f "$ici/SHA256SUMS" ]; then
    (cd "$ici" && grep ' \*\{0,1\}tirage-serveur$' SHA256SUMS | sha256sum -c --quiet -) || echec "empreinte différente de SHA256SUMS."
    echo "   conforme"
else
    echo "   SHA256SUMS absent : comparer à la main avec la page de la version : $(sha256sum "$ici/tirage-serveur" | cut -d' ' -f1)"
fi
"$ici/tirage-serveur" --version >/dev/null || echec "le programme ne démarre pas sur cette machine."

if [ -f "$conf" ]; then
    echo "2. Mise à jour : $conf existe, il est gardé tel quel"
    install -m 0755 "$ici/tirage-serveur" "$bin"
    systemctl restart tirage-serveur
    sleep 2
    systemctl is-active --quiet tirage-serveur || echec "le service ne redémarre pas : journalctl -u tirage-serveur -n 50"
    echo "Fini : $("$bin" --version), service redémarré."
    exit 0
fi

echo "2. Compte système et dossiers"
id tirage >/dev/null 2>&1 || useradd --system --home-dir /var/lib/tirage --no-create-home --shell /usr/sbin/nologin tirage
install -d -m 0750 -o root -g tirage /etc/tirage
install -d -m 0700 -o tirage -g tirage /var/lib/tirage
install -m 0755 "$ici/tirage-serveur" "$bin"

echo "3. Collecteur"
printf "   Nom du collecteur (serveur d'impression) [impression-1] : "
read -r nom
nom=${nom:-impression-1}
case "$nom" in *[!A-Za-z0-9._-]*|"") echec "nom invalide : lettres, chiffres, . _ - seulement." ;; esac
jeton=$("$bin" --nouveau-jeton "$nom" 2>/dev/null)
empreinte=$(printf %s "$jeton" | sha256sum | cut -d' ' -f1)

umask 027
cat > "$conf" <<EOF
# Configuration du serveur tirage. Modèle complet et commenté : serveur.exemple.toml de l'archive.
# Le serveur parle HTTP sur la boucle locale ; un mandataire inverse HTTPS (nginx…) le publie.
ecoute = "127.0.0.1:8080"
base = "/var/lib/tirage/tirage.sqlite"

[[collecteurs]]
nom = "$nom"
jeton_sha256 = "$empreinte"

# L'interface web s'ouvre avec la section [annuaire] (connexion LDAP / Active Directory, un groupe autorisé) :
# la recopier depuis serveur.exemple.toml, puis : systemctl restart tirage-serveur
EOF
chown root:tirage "$conf"
chmod 0640 "$conf"

echo "4. Service"
install -m 0644 "$ici/tirage-serveur.service" "$unite"
systemctl daemon-reload
systemctl enable --now tirage-serveur
sleep 2
systemctl is-active --quiet tirage-serveur || echec "le service ne démarre pas : journalctl -u tirage-serveur -n 50"

cat <<EOF

Fini : serveur installé, il reçoit les travaux sur 127.0.0.1:8080.

Jeton du collecteur « $nom » (affiché une seule fois, à garder dans un coffre) :

  $jeton

Ensuite :
  1. Publier le serveur en HTTPS : nginx-tirage.conf.exemple de l'archive.
  2. Sur le serveur d'impression : installer-collecteur.ps1 -Serveur https://<ce serveur> -Nom $nom, puis coller le jeton.
  3. Ouvrir l'interface : ajouter [annuaire] dans $conf (voir serveur.exemple.toml).
Guide complet : https://github.com/lugio/tirage/blob/main/docs/guides/installation.md
EOF
