-- Down: drop digest.digest_tips table
DROP TABLE IF EXISTS digest.digest_tips CASCADE;
DROP FUNCTION IF EXISTS digest.digest_tips_audit_timestamp() CASCADE;
