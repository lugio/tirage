//! Service Windows du collecteur : installation, désinstallation, exécution sous le gestionnaire de services.
//!
//! Le service tourne sous le compte LocalSystem (les notifications de toutes les files du serveur d'impression
//! l'exigent), démarre automatiquement (en différé) après le spouleur, et redémarre seul après un échec
//! (60 s, trois fois, compteur remis à zéro chaque jour). Sa ligne de commande porte le chemin de la configuration.

#![allow(unsafe_code)] // la macro `define_windows_service!` génère le point d'entrée appelé par Windows.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept, ServiceDependency,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo,
    ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::config::Config;
use crate::execution::{self, Reglages};
use crate::journal::Journal;

pub const NOM: &str = "tirage-collecteur";
const NOM_AFFICHE: &str = "Tirage — collecteur des impressions";
const DESCRIPTION: &str = "Relève les travaux du spouleur (utilisateur, pages, couleur, recto-verso…) et les envoie au serveur Tirage.";

static CONFIG: OnceLock<PathBuf> = OnceLock::new();

define_windows_service!(point_d_entree, service_main);

/// Appelé par `--service --config FICHIER` (ligne de commande enregistrée à l'installation).
pub fn lancer(config: PathBuf) -> Result<(), String> {
    let _ = CONFIG.set(config);
    service_dispatcher::start(NOM, point_d_entree).map_err(|e| {
        format!("lancement hors du gestionnaire de services ? ({e}) — pour un essai, utiliser --console")
    })
}

fn service_main(_args: Vec<OsString>) {
    // Sans configuration lisible, rien à faire : le service s'arrête en erreur (visible dans le gestionnaire).
    let _ = executer_service();
}

fn executer_service() -> Result<(), String> {
    let arret = Arc::new(AtomicBool::new(false));
    let a = Arc::clone(&arret);
    let poignee = service_control_handler::register(NOM, move |ordre| match ordre {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            a.store(true, Ordering::Relaxed);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })
    .map_err(|e| e.to_string())?;
    let etat = |state, code: u32, attente: Duration| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: attente,
        process_id: None,
    };
    let _ = poignee.set_service_status(etat(ServiceState::Running, 0, Duration::ZERO));

    let resultat = (|| {
        let chemin = CONFIG
            .get()
            .ok_or("configuration absente de la ligne de commande")?;
        let r = Reglages::de_config(Config::charger(chemin)?);
        let journal = Arc::new(Journal::fichier(&r.chemin_journal(), false).map_err(|e| e.to_string())?);
        let res = execution::executer(&r, &arret, &journal, false);
        if let Err(e) = &res {
            journal.ecrire(&format!("le service s'arrête en erreur : {e}"));
        }
        res
    })();

    let _ = poignee.set_service_status(etat(ServiceState::StopPending, 0, Duration::from_secs(10)));
    // Code 1 = échec : le gestionnaire applique alors les actions de redémarrage.
    let code = u32::from(resultat.is_err() && !arret.load(Ordering::Relaxed));
    let _ = poignee.set_service_status(etat(ServiceState::Stopped, code, Duration::ZERO));
    resultat
}

/// Installe le service (administrateur). La configuration doit être lisible et valide.
pub fn installer(config: &Path) -> Result<(), String> {
    let config = std::fs::canonicalize(config).map_err(|e| format!("{} : {e}", config.display()))?;
    Config::charger(&config)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let gestionnaire = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| format!("gestionnaire de services (administrateur requis) : {e}"))?;
    let info = ServiceInfo {
        name: NOM.into(),
        display_name: NOM_AFFICHE.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![
            "--service".into(),
            "--config".into(),
            chemin_windows(&config).into(),
        ],
        dependencies: vec![ServiceDependency::Service("Spooler".into())],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let s = gestionnaire
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .map_err(|e| format!("création du service : {e}"))?;
    s.set_description(DESCRIPTION).map_err(|e| e.to_string())?;
    s.set_delayed_auto_start(true).map_err(|e| e.to_string())?;
    let relance = ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(60),
    };
    s.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![relance.clone(), relance.clone(), relance]),
    })
    .map_err(|e| e.to_string())?;
    s.set_failure_actions_on_non_crash_failures(true)
        .map_err(|e| e.to_string())?;
    println!("service « {NOM} » installé (démarrage automatique différé, LocalSystem).");
    println!("Démarrer : Start-Service {NOM}    Journal : <donnees>\\journal\\collecteur.log");
    Ok(())
}

/// Arrête puis supprime le service (administrateur).
pub fn desinstaller() -> Result<(), String> {
    let gestionnaire = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| format!("gestionnaire de services : {e}"))?;
    let s = gestionnaire
        .open_service(
            NOM,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        )
        .map_err(|e| format!("service « {NOM} » introuvable : {e}"))?;
    if s.query_status().map_err(|e| e.to_string())?.current_state != ServiceState::Stopped {
        s.stop().map_err(|e| format!("arrêt : {e}"))?;
        for _ in 0..30 {
            std::thread::sleep(Duration::from_secs(1));
            if s.query_status()
                .map(|e| e.current_state == ServiceState::Stopped)
                .unwrap_or(false)
            {
                break;
            }
        }
    }
    s.delete().map_err(|e| format!("suppression : {e}"))?;
    println!("service « {NOM} » supprimé. Les données (file, journal) et la configuration restent en place.");
    Ok(())
}

/// Chemin sans le préfixe `\\?\` de `canonicalize`, lisible dans la console des services.
fn chemin_windows(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
}
