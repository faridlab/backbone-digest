-- Down: remove the company RLS fence for digest module

-- Reverse the company RLS fence for digest.digest_digests
DROP POLICY IF EXISTS digest_digests_company_isolation ON digest.digest_digests;
ALTER TABLE digest.digest_digests NO FORCE ROW LEVEL SECURITY;
ALTER TABLE digest.digest_digests DISABLE ROW LEVEL SECURITY;

