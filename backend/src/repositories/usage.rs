//! Consommation d'une org : membres, documents, questions du mois.
//! Le mois courant est calculé par Postgres, en UTC, partout pareil.

use sea_orm::{ConnectionTrait, DbErr, Statement};
use uuid::Uuid;

/// Premier jour du mois courant (UTC), en SQL.
const CURRENT_PERIOD: &str = "date_trunc('month', now() AT TIME ZONE 'UTC')::date";

/// Compte une question répondue pour le mois courant.
pub async fn increment_questions<C: ConnectionTrait>(conn: &C, org_id: Uuid) -> Result<(), DbErr> {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        format!(
            "INSERT INTO org_usage (org_id, period, questions) VALUES ($1, {CURRENT_PERIOD}, 1)
             ON CONFLICT (org_id, period) DO UPDATE SET questions = org_usage.questions + 1"
        ),
        [org_id.into()],
    ))
    .await?;
    Ok(())
}

pub struct Usage {
    /// Mois courant, `AAAA-MM`.
    pub period: String,
    pub members: i64,
    pub documents: i64,
    pub questions_this_month: i64,
}

pub async fn current<C: ConnectionTrait>(conn: &C, org_id: Uuid) -> Result<Usage, DbErr> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            conn.get_database_backend(),
            format!(
                "SELECT to_char({CURRENT_PERIOD}, 'YYYY-MM') AS period,
                    (SELECT count(*) FROM memberships WHERE org_id = $1) AS members,
                    (SELECT count(*) FROM documents WHERE org_id = $1) AS documents,
                    COALESCE((SELECT questions::bigint FROM org_usage
                              WHERE org_id = $1 AND period = {CURRENT_PERIOD}), 0) AS questions"
            ),
            [org_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("usage".into()))?;
    Ok(Usage {
        period: row.try_get("", "period")?,
        members: row.try_get("", "members")?,
        documents: row.try_get("", "documents")?,
        questions_this_month: row.try_get("", "questions")?,
    })
}
