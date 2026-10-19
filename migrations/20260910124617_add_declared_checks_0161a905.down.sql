-- Down: drop the declared rule constraints for digest module
DO $$
BEGIN
    IF to_regclass('digest.uq_digest_tip_users_user_id__unique') IS NOT NULL AND to_regclass('digest.idx_digest_tip_users_tip_id_user_id') IS NULL THEN
        ALTER INDEX digest.uq_digest_tip_users_user_id__unique RENAME TO idx_digest_tip_users_tip_id_user_id;
    END IF;
END $$;

ALTER TABLE IF EXISTS digest.digest_tips DROP CONSTRAINT IF EXISTS ck_digest_tips_group_key_max_length__max_length;
ALTER TABLE IF EXISTS digest.digest_tips DROP CONSTRAINT IF EXISTS ck_digest_tips_name_max_length__max_length;
ALTER TABLE IF EXISTS digest.digest_tips DROP CONSTRAINT IF EXISTS ck_digest_tips_name_required__required;
ALTER TABLE IF EXISTS digest.digest_digest_kpis DROP CONSTRAINT IF EXISTS ck_digest_digest_kpis_kpi_key_max_length__max_length;
ALTER TABLE IF EXISTS digest.digest_digest_kpis DROP CONSTRAINT IF EXISTS ck_digest_digest_kpis_kpi_key_required__required;
DO $$
BEGIN
    IF to_regclass('digest.uq_digest_digest_kpis_kpi_key__unique') IS NOT NULL AND to_regclass('digest.idx_digest_digest_kpis_digest_id_kpi_key') IS NULL THEN
        ALTER INDEX digest.uq_digest_digest_kpis_kpi_key__unique RENAME TO idx_digest_digest_kpis_digest_id_kpi_key;
    END IF;
END $$;

DO $$
BEGIN
    IF to_regclass('digest.uq_digest_subscriptions_user_id__unique') IS NOT NULL AND to_regclass('digest.idx_digest_subscriptions_digest_id_user_id') IS NULL THEN
        ALTER INDEX digest.uq_digest_subscriptions_user_id__unique RENAME TO idx_digest_subscriptions_digest_id_user_id;
    END IF;
END $$;

ALTER TABLE IF EXISTS digest.digest_digests DROP CONSTRAINT IF EXISTS ck_digest_digests_name_max_length__max_length;
ALTER TABLE IF EXISTS digest.digest_digests DROP CONSTRAINT IF EXISTS ck_digest_digests_name_required__required;
