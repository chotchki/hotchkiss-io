-- CQ.2 — a ZERO-STORAGE classification view over request_log. Adds one derived
-- column, ua_class ('bot'|'human'), inferred from the User-Agent. Query-time +
-- reversible BY DESIGN: the bot-UA ruleset lives HERE, in one compile-checked
-- place, NOT frozen into a stored column — so it can be retuned against all 90 days
-- of history whenever the rules improve. A view has no rows of its own, so the
-- fire-and-forget insert path AND the beta scrub stay untouched.
--
-- ua_class is DIRECTIONAL, not authoritative: User-Agent is trivially spoofable, so
-- this NEVER governs a primary number on its own and NEVER feeds off HTTP status (a
-- human clicking a dead link is a 404 AND a real reader). The behavioral catch for a
-- UA-spoofing scanner is the per-IP 404-fanout leaderboard (CQ.3), not this column.
--
-- CAST(... AS TEXT) pins the CASE result to a concrete type so sqlx infers TEXT.
CREATE VIEW IF NOT EXISTS request_log_view AS
SELECT
    id, ts, method, path, status, ip, user_agent, referer, duration_ms,
    CAST(
        CASE
            -- No/empty UA is overwhelmingly automated (real browsers always send one).
            WHEN user_agent IS NULL OR user_agent = '' THEN 'bot'
            -- Known bot / crawler / library / scanner / headless markers. Amend HERE
            -- (this is the single source) as new offenders show up in the log.
            WHEN lower(user_agent) LIKE '%bot%'
              OR lower(user_agent) LIKE '%crawl%'
              OR lower(user_agent) LIKE '%spider%'
              OR lower(user_agent) LIKE '%slurp%'
              OR lower(user_agent) LIKE '%curl%'
              OR lower(user_agent) LIKE '%wget%'
              OR lower(user_agent) LIKE '%python%'
              OR lower(user_agent) LIKE '%go-http%'
              OR lower(user_agent) LIKE '%java/%'
              OR lower(user_agent) LIKE '%okhttp%'
              OR lower(user_agent) LIKE '%httpx%'
              OR lower(user_agent) LIKE '%axios%'
              OR lower(user_agent) LIKE '%node-fetch%'
              OR lower(user_agent) LIKE '%headless%'
              OR lower(user_agent) LIKE '%phantomjs%'
              OR lower(user_agent) LIKE '%scrapy%'
              OR lower(user_agent) LIKE '%masscan%'
              OR lower(user_agent) LIKE '%zgrab%'
              OR lower(user_agent) LIKE '%nmap%'
              OR lower(user_agent) LIKE '%semrush%'
              OR lower(user_agent) LIKE '%ahrefs%'
              OR lower(user_agent) LIKE '%mj12%'
              OR lower(user_agent) LIKE '%dotbot%'
              OR lower(user_agent) LIKE '%facebookexternalhit%'
              OR lower(user_agent) LIKE '%feedfetcher%'
            THEN 'bot'
            ELSE 'human'
        END AS TEXT
    ) AS ua_class
FROM request_log;
