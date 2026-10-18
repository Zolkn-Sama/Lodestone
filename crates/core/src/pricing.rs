//! Tarif Pro : par siège, avec un socle mensuel minimum par organisation (BILL-05).
//! Montants en centimes HT, configurables côté plateforme.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProPricing {
    /// Prix d'un siège par mois (29 € = 2900).
    pub seat_price_cents: u32,
    /// Socle mensuel minimum de l'org (149 € = 14900).
    pub floor_cents: u32,
}

impl Default for ProPricing {
    fn default() -> Self {
        Self {
            seat_price_cents: 2900,
            floor_cents: 14900,
        }
    }
}

/// Détail d'une facture mensuelle Pro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MonthlyQuote {
    pub seats: u32,
    pub seats_total_cents: u64,
    pub total_cents: u64,
    /// Le socle s'applique (sièges × prix < socle).
    pub floor_applied: bool,
}

impl ProPricing {
    /// `max(socle, sièges × prix_siège)`.
    pub fn monthly(&self, seats: u32) -> MonthlyQuote {
        let seats_total = u64::from(seats) * u64::from(self.seat_price_cents);
        let floor = u64::from(self.floor_cents);
        MonthlyQuote {
            seats,
            seats_total_cents: seats_total,
            total_cents: seats_total.max(floor),
            floor_applied: seats_total < floor,
        }
    }

    /// Nombre de sièges couverts par le socle : jusqu'à ce nombre on paie le
    /// socle, au-delà le prix par siège prend le relais (5 pour 149 € / 29 €).
    /// C'est la borne du premier palier du prix Stripe.
    pub fn seats_covered_by_floor(&self) -> u32 {
        self.floor_cents
            .checked_div(self.seat_price_cents)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socle_puis_prix_par_siege() {
        let p = ProPricing::default();
        let totals: Vec<(u32, u64, bool)> = (1..=8)
            .map(|n| {
                let q = p.monthly(n);
                (n, q.total_cents, q.floor_applied)
            })
            .collect();
        assert_eq!(
            totals,
            vec![
                (1, 14900, true),
                (2, 14900, true),
                (3, 14900, true),
                (4, 14900, true),
                (5, 14900, true), // 5 × 29 = 145 € < 149 €
                (6, 17400, false),
                (7, 20300, false),
                (8, 23200, false),
            ]
        );
    }

    #[test]
    fn jamais_sous_le_socle() {
        let p = ProPricing::default();
        for n in 0..500 {
            let q = p.monthly(n);
            assert!(q.total_cents >= 14900);
            assert_eq!(q.total_cents, q.seats_total_cents.max(14900));
        }
    }

    #[test]
    fn seuil_du_socle() {
        assert_eq!(ProPricing::default().seats_covered_by_floor(), 5);
        let p = ProPricing {
            seat_price_cents: 2500,
            floor_cents: 10000,
        };
        assert_eq!(p.seats_covered_by_floor(), 4);
        assert_eq!(p.monthly(4).total_cents, 10000);
        assert_eq!(p.monthly(5).total_cents, 12500);
    }
}
