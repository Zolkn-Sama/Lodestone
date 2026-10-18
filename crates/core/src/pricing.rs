//! Tarif Pro : par siège, avec un socle minimum par organisation (BILL-05),
//! payable au mois ou à l'année avec remise (BILL-06).
//! Montants en centimes HT, configurables côté plateforme.

use serde::{Deserialize, Serialize};

/// Périodicité de facturation (même vocabulaire que Stripe : `month` / `year`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BillingInterval {
    Month,
    Year,
}

impl BillingInterval {
    pub fn as_str(&self) -> &'static str {
        match self {
            BillingInterval::Month => "month",
            BillingInterval::Year => "year",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "month" => Some(BillingInterval::Month),
            "year" => Some(BillingInterval::Year),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProPricing {
    /// Prix d'un siège par mois (29 € = 2900).
    pub seat_price_cents: u32,
    /// Socle mensuel minimum de l'org (149 € = 14900).
    pub floor_cents: u32,
    /// Remise sur l'engagement annuel, en % (0 à 99).
    pub annual_discount_percent: u32,
}

impl Default for ProPricing {
    fn default() -> Self {
        Self {
            seat_price_cents: 2900,
            floor_cents: 14900,
            annual_discount_percent: 15,
        }
    }
}

/// Détail d'une facture Pro pour une période.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Quote {
    pub interval: BillingInterval,
    pub seats: u32,
    pub seats_total_cents: u64,
    pub total_cents: u64,
    /// Le socle s'applique (sièges × prix < socle).
    pub floor_applied: bool,
}

/// Prix unitaires d'une période : socle et siège (ce que portent les paliers Stripe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UnitPrices {
    pub floor_cents: u64,
    pub seat_cents: u64,
}

impl ProPricing {
    /// Socle et siège pour la période. À l'année : 12 mois moins la remise,
    /// arrondis au centime (socle compris : « socle ramené à l'année »).
    pub fn unit_prices(&self, interval: BillingInterval) -> UnitPrices {
        let annualize = |monthly: u32| {
            let discount = u64::from(self.annual_discount_percent.min(99));
            // Arrondi au centime le plus proche.
            (u64::from(monthly) * 12 * (100 - discount) + 50) / 100
        };
        match interval {
            BillingInterval::Month => UnitPrices {
                floor_cents: u64::from(self.floor_cents),
                seat_cents: u64::from(self.seat_price_cents),
            },
            BillingInterval::Year => UnitPrices {
                floor_cents: annualize(self.floor_cents),
                seat_cents: annualize(self.seat_price_cents),
            },
        }
    }

    /// `max(socle, sièges × prix_siège)` pour la période.
    pub fn quote(&self, seats: u32, interval: BillingInterval) -> Quote {
        let prices = self.unit_prices(interval);
        let seats_total = u64::from(seats) * prices.seat_cents;
        Quote {
            interval,
            seats,
            seats_total_cents: seats_total,
            total_cents: seats_total.max(prices.floor_cents),
            floor_applied: seats_total < prices.floor_cents,
        }
    }

    pub fn monthly(&self, seats: u32) -> Quote {
        self.quote(seats, BillingInterval::Month)
    }

    pub fn annual(&self, seats: u32) -> Quote {
        self.quote(seats, BillingInterval::Year)
    }

    /// Nombre de sièges couverts par le socle sur la période : jusqu'à ce
    /// nombre on paie le socle, au-delà le prix par siège prend le relais.
    /// C'est la borne du premier palier du prix Stripe.
    pub fn seats_covered_by_floor(&self, interval: BillingInterval) -> u32 {
        let prices = self.unit_prices(interval);
        prices
            .floor_cents
            .checked_div(prices.seat_cents)
            .and_then(|n| u32::try_from(n).ok())
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
        for interval in [BillingInterval::Month, BillingInterval::Year] {
            let floor = p.unit_prices(interval).floor_cents;
            for n in 0..500 {
                let q = p.quote(n, interval);
                assert!(q.total_cents >= floor);
                assert_eq!(q.total_cents, q.seats_total_cents.max(floor));
            }
        }
    }

    #[test]
    fn seuil_du_socle() {
        let p = ProPricing::default();
        assert_eq!(p.seats_covered_by_floor(BillingInterval::Month), 5);
        assert_eq!(p.seats_covered_by_floor(BillingInterval::Year), 5);
        let p = ProPricing {
            seat_price_cents: 2500,
            floor_cents: 10000,
            annual_discount_percent: 15,
        };
        assert_eq!(p.seats_covered_by_floor(BillingInterval::Month), 4);
        assert_eq!(p.monthly(4).total_cents, 10000);
        assert_eq!(p.monthly(5).total_cents, 12500);
    }

    #[test]
    fn annuel_remise_et_socle_ramene_a_l_annee() {
        let p = ProPricing::default();
        // 149 € × 12 × 0,85 = 1 519,80 € ; 29 € × 12 × 0,85 = 295,80 €.
        assert_eq!(
            p.unit_prices(BillingInterval::Year),
            UnitPrices {
                floor_cents: 151_980,
                seat_cents: 29_580
            }
        );
        let q = p.annual(3);
        assert_eq!((q.total_cents, q.floor_applied), (151_980, true));
        let q = p.annual(10);
        assert_eq!((q.total_cents, q.floor_applied), (295_800, false));
        // L'année coûte toujours moins que 12 mois.
        for n in 1..200 {
            assert!(
                p.annual(n).total_cents < 12 * p.monthly(n).total_cents,
                "{n} sièges"
            );
        }
    }

    #[test]
    fn remise_configurable_et_arrondie() {
        let sans = ProPricing {
            annual_discount_percent: 0,
            ..ProPricing::default()
        };
        assert_eq!(sans.annual(1).total_cents, 12 * 14900);

        // 2 mois offerts ≈ 16,67 % : on prend 17 %. 2900 × 12 × 0,83 = 28 884.
        let p = ProPricing {
            annual_discount_percent: 17,
            ..ProPricing::default()
        };
        assert_eq!(p.unit_prices(BillingInterval::Year).seat_cents, 28_884);

        // Arrondi au centime le plus proche : 999 × 12 × 0,85 = 10 189,8 -> 10 190.
        let p = ProPricing {
            seat_price_cents: 999,
            floor_cents: 999,
            annual_discount_percent: 15,
        };
        assert_eq!(p.unit_prices(BillingInterval::Year).seat_cents, 10_190);
    }

    #[test]
    fn periodicite_texte() {
        for i in [BillingInterval::Month, BillingInterval::Year] {
            assert_eq!(BillingInterval::parse(i.as_str()), Some(i));
        }
        assert_eq!(BillingInterval::parse("week"), None);
    }
}
