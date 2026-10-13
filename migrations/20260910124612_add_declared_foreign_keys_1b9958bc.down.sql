-- Down: drop the declared foreign keys for digest module
ALTER TABLE IF EXISTS digest.digest_tip_users DROP CONSTRAINT IF EXISTS fk_digest_tip_users_tip_id;
ALTER TABLE IF EXISTS digest.digest_digest_kpis DROP CONSTRAINT IF EXISTS fk_digest_digest_kpis_digest_id;
ALTER TABLE IF EXISTS digest.digest_subscriptions DROP CONSTRAINT IF EXISTS fk_digest_subscriptions_digest_id;
