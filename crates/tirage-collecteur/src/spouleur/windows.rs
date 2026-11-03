//! Lecture des travaux auprès du spouleur Windows — partie PROPRE À WINDOWS, réduite aux appels système.
//!
//! On ouvre le serveur d'impression LOCAL (poignée sans nom d'imprimante : toutes les files) et on s'abonne aux
//! notifications de travaux. Chaque notification porte des champs (file, utilisateur, statut, DEVMODE…) que l'on
//! traduit en [`Champ`] pour le [`Suivi`] portable. Aucun contenu de travail n'est lu.
#![allow(unsafe_code)] // FFI du spouleur : chaque bloc `unsafe` est commenté.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{INVALID_HANDLE_VALUE, SYSTEMTIME, WAIT_OBJECT_0};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, DM_COLOR, DM_COPIES, DM_DUPLEX, DM_FORMNAME, DM_ORIENTATION, DM_PAPERSIZE, DM_PRINTQUALITY,
    DM_YRESOLUTION,
};
use windows::Win32::Graphics::Printing::{
    ClosePrinter, FindClosePrinterChangeNotification, FindFirstPrinterChangeNotification,
    FindNextPrinterChangeNotification, FreePrinterNotifyInfo, JOB_NOTIFY_FIELD_DATATYPE,
    JOB_NOTIFY_FIELD_DEVMODE, JOB_NOTIFY_FIELD_DOCUMENT, JOB_NOTIFY_FIELD_DRIVER_NAME,
    JOB_NOTIFY_FIELD_MACHINE_NAME, JOB_NOTIFY_FIELD_PAGES_PRINTED, JOB_NOTIFY_FIELD_PRINTER_NAME,
    JOB_NOTIFY_FIELD_STATUS, JOB_NOTIFY_FIELD_SUBMITTED, JOB_NOTIFY_FIELD_TOTAL_BYTES,
    JOB_NOTIFY_FIELD_TOTAL_PAGES, JOB_NOTIFY_FIELD_USER_NAME, JOB_NOTIFY_TYPE, OpenPrinterW,
    PRINTER_CHANGE_JOB, PRINTER_HANDLE, PRINTER_NOTIFY_INFO, PRINTER_NOTIFY_INFO_DATA,
    PRINTER_NOTIFY_INFO_DISCARDED, PRINTER_NOTIFY_OPTIONS, PRINTER_NOTIFY_OPTIONS_REFRESH,
    PRINTER_NOTIFY_OPTIONS_TYPE,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::PCWSTR;

use super::suivi::{Champ, Reglages, Suivi};
use super::temps::{maintenant_ms, systemtime_vers_ms};

const CHAMPS: [u32; 12] = [
    JOB_NOTIFY_FIELD_PRINTER_NAME,
    JOB_NOTIFY_FIELD_MACHINE_NAME,
    JOB_NOTIFY_FIELD_USER_NAME,
    JOB_NOTIFY_FIELD_DOCUMENT,
    JOB_NOTIFY_FIELD_DATATYPE,
    JOB_NOTIFY_FIELD_DRIVER_NAME,
    JOB_NOTIFY_FIELD_DEVMODE,
    JOB_NOTIFY_FIELD_STATUS,
    JOB_NOTIFY_FIELD_TOTAL_PAGES,
    JOB_NOTIFY_FIELD_PAGES_PRINTED,
    JOB_NOTIFY_FIELD_TOTAL_BYTES,
    JOB_NOTIFY_FIELD_SUBMITTED,
];

#[derive(Debug)]
pub struct ErreurSpouleur(pub String);

impl std::fmt::Display for ErreurSpouleur {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "spouleur : {}", self.0)
    }
}

impl std::error::Error for ErreurSpouleur {}

/// Surveille le spouleur jusqu'à `arret`. `rappel` reçoit le [`Suivi`] après chaque lot de notifications,
/// pour que l'appelant prenne les travaux terminés.
pub fn surveiller(arret: &AtomicBool, mut rappel: impl FnMut(&mut Suivi)) -> Result<(), ErreurSpouleur> {
    let mut champs = CHAMPS.map(|c| c as u16);
    let mut type_travaux = PRINTER_NOTIFY_OPTIONS_TYPE {
        Type: JOB_NOTIFY_TYPE as u16,
        Count: champs.len() as u32,
        pFields: champs.as_mut_ptr(),
        ..Default::default()
    };
    let mut options = PRINTER_NOTIFY_OPTIONS {
        Version: 2,
        Flags: 0,
        Count: 1,
        pTypes: &mut type_travaux,
    };

    let mut serveur = PRINTER_HANDLE::default();
    // SAFETY : nom nul = serveur d'impression local ; `serveur` est une sortie valide.
    unsafe { OpenPrinterW(PCWSTR::null(), &mut serveur, None) }
        .map_err(|e| ErreurSpouleur(format!("ouverture du serveur d'impression : {e}")))?;
    let _fermer_serveur = Garde(move || {
        // SAFETY : poignée ouverte ci-dessus, fermée une seule fois.
        let _ = unsafe { ClosePrinter(serveur) };
    });

    // SAFETY : `options` et les tableaux qu'il désigne vivent jusqu'à la fin de la fonction.
    let changement = unsafe {
        FindFirstPrinterChangeNotification(
            serveur,
            PRINTER_CHANGE_JOB,
            0,
            Some(&options as *const PRINTER_NOTIFY_OPTIONS as *const _),
        )
    };
    if changement.is_invalid() || changement == INVALID_HANDLE_VALUE {
        return Err(ErreurSpouleur("abonnement aux notifications refusé".into()));
    }
    let _fermer_abonnement = Garde(move || {
        // SAFETY : poignée de notification valide, fermée une seule fois.
        let _ = unsafe { FindClosePrinterChangeNotification(changement) };
    });

    let mut suivi = Suivi::default();
    let mut rafraichir = true; // au démarrage : état complet des files
    while !arret.load(Ordering::Relaxed) {
        if !rafraichir {
            // SAFETY : poignée valide ; attente bornée pour relire `arret`.
            if unsafe { WaitForSingleObject(changement, 1_000) } != WAIT_OBJECT_0 {
                suivi.balayer(maintenant_ms());
                rappel(&mut suivi);
                continue;
            }
        }
        options.Flags = if rafraichir {
            PRINTER_NOTIFY_OPTIONS_REFRESH
        } else {
            0
        };
        let mut info: *mut core::ffi::c_void = core::ptr::null_mut();
        let mut quoi = 0u32;
        // SAFETY : poignée valide ; `info` reçoit un bloc alloué par le spouleur, libéré plus bas.
        let ok = unsafe {
            FindNextPrinterChangeNotification(
                changement,
                Some(&mut quoi),
                Some(&options as *const PRINTER_NOTIFY_OPTIONS as *const _),
                Some(&mut info),
            )
        };
        if !ok.as_bool() {
            return Err(ErreurSpouleur("lecture des notifications impossible".into()));
        }
        let etait_rafraichi = rafraichir;
        rafraichir = false;
        if info.is_null() {
            continue;
        }
        let info = info as *const PRINTER_NOTIFY_INFO;
        // SAFETY : bloc valide jusqu'à FreePrinterNotifyInfo ; `aData` contient `Count` éléments.
        let (drapeaux, donnees) = unsafe {
            let n = (*info).Count as usize;
            (
                (*info).Flags,
                core::slice::from_raw_parts((*info).aData.as_ptr(), n),
            )
        };
        if drapeaux & PRINTER_NOTIFY_INFO_DISCARDED != 0 {
            rafraichir = true; // le spouleur a perdu des notifications : on redemande l'état complet
        } else {
            let t = maintenant_ms();
            let mut presents = Vec::new();
            for d in donnees {
                if u32::from(d.Type) != JOB_NOTIFY_TYPE {
                    continue;
                }
                presents.push(d.Id);
                if let Some(champ) = traduire(d) {
                    suivi.appliquer(d.Id, champ, t);
                }
            }
            if etait_rafraichi {
                presents.sort_unstable();
                presents.dedup();
                suivi.rafraichi(&presents);
            }
        }
        // SAFETY : bloc rendu par FindNextPrinterChangeNotification, libéré une fois.
        let _ = unsafe { FreePrinterNotifyInfo(info) };
        rappel(&mut suivi);
    }
    Ok(())
}

/// Traduit une donnée de notification en [`Champ`].
fn traduire(d: &PRINTER_NOTIFY_INFO_DATA) -> Option<Champ> {
    let champ = u32::from(d.Field);
    // SAFETY : la nature de l'union est fixée par le champ (documentation de l'API) :
    // nombres → `adwData`, chaînes / SYSTEMTIME / DEVMODE → `Data`.
    unsafe {
        let nombre = || d.NotifyData.adwData[0];
        let tampon = || (d.NotifyData.Data.pBuf, d.NotifyData.Data.cbBuf as usize);
        Some(match champ {
            JOB_NOTIFY_FIELD_PRINTER_NAME => Champ::File(chaine(tampon())?),
            JOB_NOTIFY_FIELD_MACHINE_NAME => Champ::Poste(chaine(tampon())?),
            JOB_NOTIFY_FIELD_USER_NAME => Champ::Utilisateur(chaine(tampon())?),
            JOB_NOTIFY_FIELD_DOCUMENT => Champ::Document(chaine(tampon())?),
            JOB_NOTIFY_FIELD_DATATYPE => Champ::TypeDonnees(chaine(tampon())?),
            JOB_NOTIFY_FIELD_DRIVER_NAME => Champ::Pilote(chaine(tampon())?),
            JOB_NOTIFY_FIELD_STATUS => Champ::Statut(nombre()),
            JOB_NOTIFY_FIELD_TOTAL_PAGES => Champ::PagesTotales(nombre()),
            JOB_NOTIFY_FIELD_PAGES_PRINTED => Champ::PagesImprimees(nombre()),
            JOB_NOTIFY_FIELD_TOTAL_BYTES => Champ::Octets(u64::from(nombre())),
            JOB_NOTIFY_FIELD_SUBMITTED => {
                let (p, n) = tampon();
                if p.is_null() || n < core::mem::size_of::<SYSTEMTIME>() {
                    return None;
                }
                let s = core::ptr::read_unaligned(p as *const SYSTEMTIME);
                Champ::Envoye(systemtime_vers_ms(
                    s.wYear,
                    s.wMonth,
                    s.wDay,
                    s.wHour,
                    s.wMinute,
                    s.wSecond,
                    s.wMilliseconds,
                )?)
            }
            JOB_NOTIFY_FIELD_DEVMODE => {
                let (p, n) = tampon();
                if p.is_null() || n < core::mem::size_of::<DEVMODEW>() {
                    return None;
                }
                Champ::Reglages(reglages(&core::ptr::read_unaligned(p as *const DEVMODEW)))
            }
            _ => return None,
        })
    }
}

/// Lit une chaîne UTF-16 terminée par zéro, bornée par la taille du tampon.
///
/// SAFETY (appelant) : `p` désigne `n` octets lisibles.
unsafe fn chaine((p, n): (*mut core::ffi::c_void, usize)) -> Option<String> {
    if p.is_null() || n < 2 {
        return None;
    }
    // SAFETY : voir ci-dessus ; lecture limitée à n/2 unités UTF-16.
    let unites = unsafe { core::slice::from_raw_parts(p as *const u16, n / 2) };
    let fin = unites.iter().position(|&u| u == 0).unwrap_or(unites.len());
    Some(String::from_utf16_lossy(&unites[..fin]))
}

fn reglages(dm: &DEVMODEW) -> Reglages {
    let a = dm.dmFields;
    let a_le = |f: windows::Win32::Graphics::Gdi::DEVMODE_FIELD_FLAGS| a.0 & f.0 != 0;
    // SAFETY : pour une imprimante, la première variante de l'union (orientation, papier, copies…) est la bonne.
    let p = unsafe { dm.Anonymous1.Anonymous1 };
    let format = a_le(DM_FORMNAME)
        .then(|| {
            let fin = dm
                .dmFormName
                .iter()
                .position(|&u| u == 0)
                .unwrap_or(dm.dmFormName.len());
            String::from_utf16_lossy(&dm.dmFormName[..fin]).trim().to_owned()
        })
        .filter(|s| !s.is_empty());
    let resolution = if a_le(DM_PRINTQUALITY) && p.dmPrintQuality > 0 {
        Some(p.dmPrintQuality as u32)
    } else if a_le(DM_YRESOLUTION) && dm.dmYResolution > 0 {
        Some(dm.dmYResolution as u32)
    } else {
        None
    };
    Reglages {
        copies: (a_le(DM_COPIES) && p.dmCopies > 0).then_some(p.dmCopies as u32),
        couleur: a_le(DM_COLOR).then_some(dm.dmColor.0),
        duplex: a_le(DM_DUPLEX).then_some(dm.dmDuplex.0),
        format,
        format_code: a_le(DM_PAPERSIZE).then_some(p.dmPaperSize),
        orientation: a_le(DM_ORIENTATION).then_some(p.dmOrientation),
        resolution,
    }
}

/// Exécute une fermeture à la sortie de portée.
struct Garde<F: FnMut()>(F);

impl<F: FnMut()> Drop for Garde<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}
