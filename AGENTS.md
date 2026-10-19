# Contexto para o assistente (leia antes de gerar codigo)

Monitor de publicacoes oficiais em Rust: coleta diarios oficiais por
palavra-chave **e** acompanha processos do CNJ (DataJud), grava em SQLite com
deduplicacao e serve uma pagina web com filtros.

Estado em **2026-10-19**: 3 fontes, CLI com 4 subcomandos, pagina web, imagem
Docker e deploy no Fly.io.

## Stack
- Tokio, reqwest, serde, scraper (HTML), sqlx + SQLite, clap, tracing,
  thiserror/anyhow, axum + minijinja (web).
- Configuracao em TOML (`config.toml`, gitignored) + variaveis de ambiente.

## Convencoes de codigo
- Erros de dominio com `thiserror` (`AppError`), retornados como `Result<T, AppError>`
- Erros de topo (main) com `anyhow`
- Todo I/O e async; nunca `block_on` fora do main
- Logs estruturados com `tracing`, campos nomeados
- Funcoes pequenas e testaveis; sem `unwrap()` em producao
- Comentarios em portugues, codigo em ingles
- `cargo test` e `cargo clippy --all-targets -- -D warnings` precisam passar limpo

## Arquitetura
```
src/
  main.rs       CLI (clap) e dispatch; run_scan() compartilha a logica
  config.rs     TOML + env; validacao por comando (scan vs serve)
  db.rs         pool SQLite (create_if_missing) + migrate!
  http.rs       GET com retry/backoff (fonte de diarios)
  model.rs      RawGazette, hashes, normalizacao de texto
  sources/      trait Source + querido_diario + famurs + datajud
  commands/     scan (escopos), serve (web), territories
migrations/     schema
templates/      index.html
docs/           fatos verificados de cada fonte
```

### scan
`Escopo::{Tudo, SoDiarios, SoProcessos}`. Diarios (QD + FAMURS) rodam primeiro;
DataJud roda depois, **serial** (~30 s por processo). Nunca `tokio::join!` entre
os dois. Uma fonte que falha nao derruba as outras; erro de `AppError::Config`
aborta tudo. CLI: `scan`, `scan --only-diarios`, `scan --only-processos`,
`scan-processos`.

## Fatos por fonte (ver docs/)

### Querido Diario — `docs/api-findings.md`
- Host: `https://api.queridodiario.org.br` (o `.ok.org.br/api` responde 302).
- O filtro e `querystring`, **nao** `keywords` (que e ignorado em silencio).
- Keywords: `+termo +termo` (AND), sem acento, **sem aspas** (aspas exigem
  acento exato e podem zerar em silencio). `AND` textual nao funciona.
- Resposta: `date`, `excerpts[]`, sem id — derive da `url`.
- Cobertura desigual: SP tem ~20 publicacoes; RS esta estagnado.

### FAMURS (RS) — `docs/fontes-rs.md`
- Entidade `64462` = Prefeitura de Sao Sepe; portal SIGPub, HTML server-side.
- Texto integral em `/famurs/download/materia/{codigo}` (nao precisa de PDF).
- `external_id` = codigo da materia. 500 ms entre materias.
- Santa Maria nao tem fonte acessivel (site sem acervo).

### DataJud (CNJ) — `docs/datajud-findings.md`
- `POST /api_publica_{tribunal}/_search`; chave publica na wiki do CNJ.
- A chave pode ser colada com ou sem o prefixo `APIKey ` (normalizamos).
- STF **nao existe** na API. Suportados: tjsp, tjrj, tjrs, trf4, stj.
- `external_id` por **movimento**: `{id}:{codigo}:{dataHora}`.
- `published_at` = data do **movimento** (nao `dataAjuizamento`).
- `matched_keyword` = nome do movimento; apelido vai no excerpt.
- Movimentos vem em ordem inconsistente — sempre ordenar por `dataHora`.
- API lenta (8-35 s) e instavel: 429 `es_rejected_execution_exception`,
  504 do LB em ~60 s, e `200` com `timed_out: true` (tratar como erro).
- `0 hits` **nao prova** que o processo nao existe — reconsulte.

## Regras de negocio
- `gazettes.id` = `<source>:<sha256(source|external_id)>`.
- `alerts.alert_hash` UNIQUE = sha256(source|external_id|keyword normalizada).
- A keyword fica em **`alerts.keyword`**, nao em `gazettes`. Consulta que
  queira o badge precisa de `JOIN alerts`.
- Dedup no banco com `INSERT OR IGNORE`; `gazettes` + `alerts` na mesma transacao.
- Fonte nova = modulo em `src/sources/`, implementa `Source`, registra em
  `sources/mod.rs`, e adiciona ao `build_sources_*` do `scan.rs`.

## Pagina web (`serve`)
- `/` com filtros `city`, `keyword`, `days`; `/health` para o Fly.
- Filtro de origem e um `<select>` com as origens do banco ("Todas as origens").
- Card **Fontes** = `COUNT(DISTINCT source || ':' || territory_name)`.
- Cards do topo respeitam os filtros (mesmo WHERE da lista).
- Link por fonte: diarios abrem a URL original; DataJud monta a consulta do
  tribunal. **So STJ e TJSP abrem o processo direto** (TJSP via consulta
  unificada com numero formatado). TJRJ/TJRS/TRF4 caem em
  `https://www.cnj.jus.br/consultaprocessos/` com label "Consultar no CNJ →".
- Template: `include_str!` como fallback; em debug o HTML e lido do disco a cada
  request (hot-reload). Em release, editar o template exige `cargo build`.

## Armadilhas ja pagas
- **`.env` com `DATABASE_URL` sobrescreve** o `database_url` do `--config`.
  Para usar outro banco, exporte `DATABASE_URL` explicitamente.
- No Fly, o `DATABASE_URL` vem de `ENV` na imagem (nao do `start.sh`): um
  `fly ssh console -C "monitor-diarios scan ..."` so escreve no volume se o
  processo herdar essa env. Sem ela, grava em `/app/monitor.db` (efemero) e a
  pagina nao ve os dados.
- `SqlitePool::connect` falha se o arquivo nao existe — usamos
  `create_if_missing(true)`.
- `date('now')` e UTC: a janela usa `date('now','-3 hours', ...)`.
- Chave publica do DataJud em config + env: env tem precedencia.
- Nao commitar `config.toml`, `monitor.db`, `.env`, `.tools/`, `.cargo-home/`.

## Nao faca
- Nao inventar endpoints ou campos: teste e reporte o corpo da resposta.
- Nao prometer link que abre a home do tribunal.
- Nao usar libs pesadas — o binario deve rodar em ~20 MB de RAM.
- Nao implementar Telegram/agendador sem pedido explicito.
