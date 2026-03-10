use rusqlite::Connection;

/// Run all schema migrations. Idempotent.
pub fn run_migrations(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        -- Enable WAL mode for concurrent reads during writes.
        PRAGMA journal_mode = WAL;
        -- Enforce foreign keys.
        PRAGMA foreign_keys = ON;
        -- Secure delete: overwrite deleted data with zeros.
        PRAGMA secure_delete = ON;

        CREATE TABLE IF NOT EXISTS sessions (
            key             TEXT PRIMARY KEY NOT NULL,
            agent_id        TEXT NOT NULL,
            channel         TEXT NOT NULL,
            account_id      TEXT NOT NULL,
            scoping         TEXT NOT NULL DEFAULT 'main',
            thread_id       TEXT,
            metadata        TEXT NOT NULL DEFAULT '{}',
            created_at      TEXT NOT NULL,
            last_message_at TEXT,

            -- Indexes for common queries
            CHECK(length(key) > 0)
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_agent
            ON sessions(agent_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_channel
            ON sessions(channel, account_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_last_message
            ON sessions(last_message_at);

        CREATE TABLE IF NOT EXISTS transcript (
            session_key TEXT    NOT NULL,
            seq         INTEGER NOT NULL,
            role        TEXT    NOT NULL,
            content     BLOB   NOT NULL,  -- Encrypted if encryption enabled
            metadata    TEXT,
            timestamp   TEXT    NOT NULL,

            PRIMARY KEY (session_key, seq),
            FOREIGN KEY (session_key) REFERENCES sessions(key) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_transcript_session
            ON transcript(session_key, seq DESC);
        ",
    )?;
    Ok(())
}
