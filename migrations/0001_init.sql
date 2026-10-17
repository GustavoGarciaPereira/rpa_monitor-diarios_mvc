-- Publicacoes coletadas. O id e deterministico:
--   "<source>:<sha256(source|external_id)>"
-- Assim, rodar o monitor duas vezes nunca duplica a mesma publicacao.
CREATE TABLE IF NOT EXISTS gazettes (
    id               TEXT    PRIMARY KEY,
    source           TEXT    NOT NULL,
    external_id      TEXT    NOT NULL,
    territory_id     TEXT    NOT NULL,
    territory_name   TEXT    NOT NULL,
    state_code       TEXT    NOT NULL,
    published_at     DATE    NOT NULL,
    url              TEXT    NOT NULL,
    txt_url          TEXT,
    edition          TEXT,
    is_extra_edition INTEGER NOT NULL DEFAULT 0,
    excerpt          TEXT,
    found_at         TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (source, external_id)
);

-- Um alerta por (publicacao, palavra-chave).
-- alert_hash = sha256(source|external_id|keyword_normalizada) e o UNIQUE
-- que garante a deduplicacao entre execucoes.
CREATE TABLE IF NOT EXISTS alerts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    gazette_id TEXT    NOT NULL REFERENCES gazettes(id) ON DELETE CASCADE,
    keyword    TEXT    NOT NULL,
    alert_hash TEXT    NOT NULL UNIQUE,
    sent_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    notified   INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_gazettes_published ON gazettes (published_at);
CREATE INDEX IF NOT EXISTS idx_gazettes_territory ON gazettes (territory_id);
CREATE INDEX IF NOT EXISTS idx_alerts_gazette     ON alerts (gazette_id);
