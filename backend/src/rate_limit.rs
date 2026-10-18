//! Limitation des tentatives (connexion, inscription, mot de passe oublié).
//!
//! Fenêtre glissante en mémoire : suffisant pour une instance unique (MVP).
//! Avec plusieurs instances, chaque instance compte de son côté.

use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Au-delà, on fait le ménage des clés inactives pour borner la mémoire.
const MAX_KEYS_BEFORE_CLEANUP: usize = 10_000;

/// Au plus `max` événements par clé sur `window`.
pub struct RateLimiter {
    max: usize,
    window: Duration,
    events: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            events: Mutex::default(),
        }
    }

    /// `Err(attente)` si la limite est atteinte pour `key`.
    pub fn check(&self, key: &str) -> Result<(), Duration> {
        self.check_at(key, Instant::now())
    }

    /// Compte un événement pour `key`.
    pub fn record(&self, key: &str) {
        self.record_at(key, Instant::now());
    }

    /// Oublie `key` (ex. connexion réussie : les échecs passés ne comptent plus).
    pub fn reset(&self, key: &str) {
        if let Ok(mut events) = self.events.lock() {
            events.remove(key);
        }
    }

    /// Vérifie puis compte, d'un coup (pour les actions limitées en volume).
    pub fn hit(&self, key: &str) -> Result<(), Duration> {
        let now = Instant::now();
        self.check_at(key, now)?;
        self.record_at(key, now);
        Ok(())
    }

    fn check_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let Ok(mut events) = self.events.lock() else {
            return Ok(()); // verrou empoisonné : ne pas bloquer les utilisateurs
        };
        let Some(times) = events.get_mut(key) else {
            return Ok(());
        };
        prune(times, now, self.window);
        if times.len() >= self.max {
            // Disponible quand le plus ancien événement sort de la fenêtre.
            let oldest = times.front().copied().unwrap_or(now);
            return Err(self.window.saturating_sub(now.duration_since(oldest)));
        }
        Ok(())
    }

    fn record_at(&self, key: &str, now: Instant) {
        let Ok(mut events) = self.events.lock() else {
            return;
        };
        if events.len() > MAX_KEYS_BEFORE_CLEANUP {
            events.retain(|_, times| {
                prune(times, now, self.window);
                !times.is_empty()
            });
        }
        let times = events.entry(key.to_string()).or_default();
        prune(times, now, self.window);
        times.push_back(now);
    }
}

fn prune(times: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while times
        .front()
        .is_some_and(|t| now.duration_since(*t) >= window)
    {
        times.pop_front();
    }
}

/// Les limites de l'application.
pub struct Limits {
    /// Échecs de connexion par (compte, IP) : 5 par 15 min.
    pub login_failures: RateLimiter,
    /// Tentatives de connexion par IP, tous comptes confondus : 50 par 15 min.
    pub login_per_ip: RateLimiter,
    /// Inscriptions par IP : 20 par heure.
    pub register_per_ip: RateLimiter,
    /// Demandes de réinitialisation par e-mail : 3 par heure.
    pub password_reset_per_email: RateLimiter,
    /// Demandes de réinitialisation par IP : 10 par heure.
    pub password_reset_per_ip: RateLimiter,
}

impl Default for Limits {
    fn default() -> Self {
        let minutes = |m: u64| Duration::from_secs(m * 60);
        Self {
            login_failures: RateLimiter::new(5, minutes(15)),
            login_per_ip: RateLimiter::new(50, minutes(15)),
            register_per_ip: RateLimiter::new(20, minutes(60)),
            password_reset_per_email: RateLimiter::new(3, minutes(60)),
            password_reset_per_ip: RateLimiter::new(10, minutes(60)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limite_puis_fenetre_glissante() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        for i in 0..3 {
            assert!(limiter.check_at("k", t0).is_ok(), "événement {i}");
            limiter.record_at("k", t0 + Duration::from_secs(i * 10));
        }
        // 3 événements à t0, t0+10, t0+20 : bloqué, jusqu'à t0+60.
        let wait = limiter
            .check_at("k", t0 + Duration::from_secs(30))
            .expect_err("bloqué");
        assert_eq!(wait, Duration::from_secs(30));
        assert!(
            limiter.check_at("k", t0 + Duration::from_secs(60)).is_ok(),
            "le premier est sorti"
        );
    }

    #[test]
    fn cles_independantes_et_reset() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        let now = Instant::now();
        limiter.record_at("alice", now);
        assert!(limiter.check_at("alice", now).is_err());
        assert!(
            limiter.check_at("bob", now).is_ok(),
            "une autre clé n'est pas touchée"
        );
        limiter.reset("alice");
        assert!(limiter.check_at("alice", now).is_ok());
    }

    #[test]
    fn hit_verifie_et_compte() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.hit("k").is_ok());
        assert!(limiter.hit("k").is_ok());
        assert!(limiter.hit("k").is_err());
    }

    #[test]
    fn menage_des_cles_inactives() {
        let limiter = RateLimiter::new(1, Duration::from_secs(1));
        let t0 = Instant::now();
        for i in 0..=MAX_KEYS_BEFORE_CLEANUP {
            limiter.record_at(&format!("k{i}"), t0);
        }
        // Bien plus tard : un nouvel enregistrement déclenche le ménage.
        limiter.record_at("nouveau", t0 + Duration::from_secs(10));
        let len = limiter.events.lock().map(|e| e.len()).unwrap_or(usize::MAX);
        assert_eq!(len, 1);
    }
}
