-- Hand-authored (user-owned). Not regenerated.
--
-- Strip every company-fence artifact from the digest tables (ADR-0029): the module is
-- tenant-agnostic; org scoping is installed by the COMPOSING service's tenancy decorator,
-- never by the module. Dropped here, from digest.digest_digests — the only table that
-- ever carried the fence: the company-leading index, the digest_digests_company_isolation
-- RLS policy, and the company_id column itself. The through-tables (digest_subscriptions,
-- digest_digest_kpis, digest_tips, digest_tip_users) never had a company column.
--
-- Ordering guard (the decorator must run FIRST on any database with data): the module
-- never moves tenancy data. The table is safe to strip when EITHER
--   a) it carries org_unit_id with no NULLs — the decorator backfilled it from company_id —
--      or b) it is empty (a fresh database: the earlier chain files created it empty).
-- Otherwise the strip RAISEs, naming the decorator step, rather than dropping a column
-- that still holds the only tenancy key. The file is re-runnable (every drop is IF EXISTS
-- and the tracker has no checksums), so a failed run retries cleanly after the decorator
-- lands.
--
-- RLS enable/force flags are deliberately NOT touched: the decorator owns those now.

DO $$
DECLARE
    has_org boolean;
    org_nulls bigint;
    total bigint;
BEGIN
    IF to_regclass('digest.digest_digests') IS NULL THEN
        RETURN; -- chain not fully applied on this database; nothing to strip
    END IF;

    SELECT EXISTS (
               SELECT 1 FROM information_schema.columns
               WHERE table_schema = 'digest' AND table_name = 'digest_digests' AND column_name = 'org_unit_id'
           )
    INTO has_org;

    EXECUTE 'SELECT count(*) FROM digest.digest_digests' INTO total;

    IF has_org THEN
        EXECUTE 'SELECT count(*) FROM digest.digest_digests WHERE org_unit_id IS NULL'
        INTO org_nulls;
    ELSE
        org_nulls := total; -- no org column: every row's only tenancy key is company_id
    END IF;

    IF NOT ((has_org AND org_nulls = 0) OR total = 0) THEN
        RAISE EXCEPTION 'refusing to strip company_id — digest.digest_digests is not yet covered by the tenancy decorator (% rows, % rows not covered by org_unit_id). Apply the composing service''s tenancy decorator (it backfills org_unit_id from company_id) and re-run; it is the only step that moves tenancy data.', total, org_nulls;
    END IF;
END $$;

-- ── digest_digests ─────────────────────────────────────────────────────────────
DROP INDEX IF EXISTS digest.idx_digest_digests_company_id;
DROP POLICY IF EXISTS digest_digests_company_isolation ON digest.digest_digests;
ALTER TABLE digest.digest_digests DROP COLUMN IF EXISTS company_id;
