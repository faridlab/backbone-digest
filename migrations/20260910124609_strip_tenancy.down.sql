-- Hand-authored (user-owned). Not regenerated.
--
-- Best-effort restore sketch for the tenancy strip (ADR-0029). This is a breaking module
-- release against dev-stage databases: the down re-adds the company_id column as nullable
-- with its plain index and the company isolation policy shape, but restores NO data —
-- rows written after the strip (or after the decorator re-keyed them) carry org_unit_id
-- only, and a NOT NULL company_id would refuse them outright. The composing service's
-- tenancy decorator remains the live fence; treat this down as a schema-shape sketch for
-- archaeology, not a usable rollback.

ALTER TABLE digest.digest_digests ADD COLUMN IF NOT EXISTS company_id uuid;

CREATE INDEX IF NOT EXISTS idx_digest_digests_company_id ON digest.digest_digests (company_id);

CREATE POLICY digest_digests_company_isolation ON digest.digest_digests
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
