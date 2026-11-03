//! Conversions de dates, portables.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn maintenant_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Date UTC (champs d'un SYSTEMTIME) → millisecondes depuis l'époque Unix. `None` si la date est invalide.
pub fn systemtime_vers_ms(an: u16, mois: u16, jour: u16, h: u16, mi: u16, s: u16, ms: u16) -> Option<i64> {
    if !(1..=12).contains(&mois) || !(1..=31).contains(&jour) || h > 23 || mi > 59 || s > 60 || ms > 999 {
        return None;
    }
    // Jours depuis le 1970-01-01 (algorithme « days from civil », calendrier grégorien proleptique).
    let (y, m, d) = (i64::from(an), i64::from(mois), i64::from(jour));
    let y = if m <= 2 { y - 1 } else { y };
    let ere = y.div_euclid(400);
    let ade = y - ere * 400;
    let mp = (m + 9) % 12;
    let adj = (153 * mp + 2) / 5 + d - 1;
    let ade_jours = ade * 365 + ade / 4 - ade / 100 + adj;
    let jours = ere * 146_097 + ade_jours - 719_468;
    Some(((jours * 24 + i64::from(h)) * 60 + i64::from(mi)) * 60_000 + i64::from(s) * 1000 + i64::from(ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_connues() {
        assert_eq!(systemtime_vers_ms(1970, 1, 1, 0, 0, 0, 0), Some(0));
        assert_eq!(
            systemtime_vers_ms(2026, 1, 1, 0, 0, 0, 0),
            Some(1_767_225_600_000)
        );
        assert_eq!(
            systemtime_vers_ms(2024, 2, 29, 12, 30, 15, 250),
            Some(1_709_209_815_250)
        );
        assert_eq!(systemtime_vers_ms(2000, 3, 1, 0, 0, 0, 0), Some(951_868_800_000));
    }

    #[test]
    fn invalides() {
        assert_eq!(systemtime_vers_ms(2026, 13, 1, 0, 0, 0, 0), None);
        assert_eq!(systemtime_vers_ms(2026, 1, 0, 0, 0, 0, 0), None);
        assert_eq!(systemtime_vers_ms(2026, 1, 1, 24, 0, 0, 0), None);
    }
}
