-- Down: drop the declared rule constraints for digest module
ALTER TABLE IF EXISTS digest.digest_tips DROP CONSTRAINT IF EXISTS ck_digest_tips_group_key_max_length__max_length;
ALTER TABLE IF EXISTS digest.digest_tips DROP CONSTRAINT IF EXISTS ck_digest_tips_name_max_length__max_length;
ALTER TABLE IF EXISTS digest.digest_tips DROP CONSTRAINT IF EXISTS ck_digest_tips_name_required__required;
ALTER TABLE IF EXISTS digest.digest_digest_kpis DROP CONSTRAINT IF EXISTS ck_digest_digest_kpis_kpi_key_max_length__max_length;
ALTER TABLE IF EXISTS digest.digest_digest_kpis DROP CONSTRAINT IF EXISTS ck_digest_digest_kpis_kpi_key_required__required;
ALTER TABLE IF EXISTS digest.digest_digests DROP CONSTRAINT IF EXISTS ck_digest_digests_name_max_length__max_length;
ALTER TABLE IF EXISTS digest.digest_digests DROP CONSTRAINT IF EXISTS ck_digest_digests_name_required__required;
