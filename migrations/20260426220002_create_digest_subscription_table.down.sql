-- Down: drop digest.digest_subscriptions table
DROP TABLE IF EXISTS digest.digest_subscriptions CASCADE;
DROP FUNCTION IF EXISTS digest.digest_subscriptions_audit_timestamp() CASCADE;
