# Adding a printer brand / Ajouter une marque de copieur

tirage reads copier counters over SNMP (read-only). Any device gets its **total** page count from the standard
Printer-MIB; splitting **black and colour** needs a small **profile** for the brand. A profile is a text file: no Rust
needed, and it can be tried on your own device before it is shared.

*Résumé en français en fin de page.*

## 1. Find the brand number

Every SNMP device announces an identifier, `sysObjectID`, under `1.3.6.1.4.1.<enterprise>`: the enterprise number
(IANA) is the brand. Declare the copier in a test configuration (see
[the collector trial guide](essai-collecteur.md)), then:

```powershell
.\tirage-collecteur.exe --parcourir 1.3.6.1.2.1.1.2 --copieur COPIEUR-EXEMPLE-1 --config C:\essai-tirage\collecteur.toml
# 1.3.6.1.2.1.1.2.0 = OID: 1.3.6.1.4.1.32473.1.2.3   ← enterprise 32473
```

With net-snmp, `snmpget -On -v2c -c <community> <address> 1.3.6.1.2.1.1.2.0` gives the same line.

## 2. Walk the brand's private tree

```powershell
.\tirage-collecteur.exe --parcourir 1.3.6.1.4.1.32473 --max 5000 --copieur COPIEUR-EXEMPLE-1 --config C:\essai-tirage\collecteur.toml > parcours.txt
```

(or `snmpwalk -On -v2c -c <community> <address> 1.3.6.1.4.1.32473 > parcours.txt`). **At the same moment**, with no job
in between, write down the counters shown by the copier itself (control panel or web page « counters »): total, black,
colour, and if possible copy / print separately.

> ⚠️ `parcours.txt` describes the device and sometimes its users (serial number, name, location, addresses, on some
> brands account names or job logs). **Keep it private.** Only the few numeric lines you select in step 4 are ever
> shared.

## 3. Write the profile

Look for the panel's numbers in `parcours.txt`. A brand often splits a counter (copy + print, several colour modes):
the profile lists the OIDs to **add up**. Create `crates/tirage-collecteur/profils/<name>.toml`:

```toml
nom = "example-brand"                 # lower case, digits and dashes; same as the file name
description = "Example Brand (Model series)"
entreprise = 32473                    # chosen automatically when sysObjectID is under 1.3.6.1.4.1.32473

[compteurs]
total = []                            # empty = Printer-MIB total (recommended)
noir = ["1.3.6.1.4.1.32473.1.10.1", "1.3.6.1.4.1.32473.1.10.2"]
couleur = ["1.3.6.1.4.1.32473.1.11.1"]
absents_toleres = true                # an OID missing or negative counts as 0 (e.g. no copy function)
```

Rules: one profile per brand (enterprise number); a counter is never published from a partial sum, unless
`absents_toleres` says that a missing OID really means « not available on this model »; comment the OIDs (which
function, which mode, which models you checked).

Try it on your device **without recompiling**: put the file in a folder, set `dossier_profils` under `[snmp]` in the
test configuration, then run `--releve`: the log names the profile used and the JSON shows the counters.

## 4. Add the test sample

Create `crates/tirage-collecteur/profils/essais/<name>.toml`, with **only** the `sysObjectID` line and the lines the
profile reads, copied from `parcours.txt` (numbers may be changed; keep them consistent):

```toml
profil = "example-brand"
appareil = "Example Brand C300"       # brand and model, never a serial number

releve = """
1.3.6.1.2.1.1.2.0 = OID: 1.3.6.1.4.1.32473.1.2.3
1.3.6.1.2.1.43.10.2.1.4.1.1 = Counter32: 15000
1.3.6.1.4.1.32473.1.10.1 = Counter32: 4000
1.3.6.1.4.1.32473.1.10.2 = Counter32: 6000
1.3.6.1.4.1.32473.1.11.1 = Counter32: 5000
"""

[attendu]
total = 15000
noir = 10000
couleur = 5000
# ecart_max = 15                      # if black + colour differs slightly from the total on real devices
```

The test **refuses** text, hex strings, addresses and time ticks in a sample, so that a contributed sample cannot leak
anything about a device. Run it:

```sh
cargo test -p tirage-collecteur --test profils
```

It checks that every profile has its sample, that the profile is chosen automatically from `sysObjectID`, that the
counters are the expected ones, and that black + colour equals the total.

## 5. Open a pull request

Title `feat(profils): <brand>`. Say which models you checked and how (panel counters at the time of the walk). See
[CONTRIBUTING.md](../../CONTRIBUTING.md), including the contributor licence agreement.

**Can't do it yourself?** Open a [« Printer profile » issue](https://github.com/lugio/tirage/issues/new?template=printer_profile.yml)
with the brand, the model, the enterprise number and the panel counters; numeric lines are welcome, the full walk is not.

---

## Résumé en français

Toute marque donne son **total** par la Printer-MIB standard ; le partage **noir / couleur** demande un **profil**,
simple fichier texte.

1. Trouver le numéro d'entreprise de la marque : `--parcourir 1.3.6.1.2.1.1.2` (ou `snmpget`).
2. Parcourir la MIB privée (`--parcourir 1.3.6.1.4.1.<entreprise> --max 5000 > parcours.txt`) et noter **au même
   moment** les compteurs affichés par le copieur. `parcours.txt` décrit l'appareil (et parfois ses utilisateurs) :
   il reste **privé**.
3. Écrire `profils/<nom>.toml` : les OID à **additionner** pour le noir et la couleur ; l'essayer sur son appareil
   sans recompiler (`dossier_profils`, puis `--releve`).
4. Ajouter `profils/essais/<nom>.toml` : l'identifiant de l'appareil et les seules lignes lues, plus les compteurs
   attendus. Le test refuse tout texte ou adresse ; `cargo test -p tirage-collecteur --test profils`.
5. Ouvrir une PR `feat(profils): <marque>`, ou à défaut un ticket « Printer profile ».
