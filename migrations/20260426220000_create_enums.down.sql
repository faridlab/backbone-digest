-- Down: drop enum types for digest module
DROP TYPE IF EXISTS digest_subscription_state CASCADE;
DROP TYPE IF EXISTS digest_state CASCADE;
DROP TYPE IF EXISTS digest_periodicity CASCADE;
