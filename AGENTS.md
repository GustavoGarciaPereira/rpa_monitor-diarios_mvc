# Contexto para o assistente (leia antes de gerar codigo)

Projeto: daemon CLI em Rust que monitora diarios oficiais e alerta sobre
novas publicacoes que casem com palavras-chave configuradas.

## Stack obrigatoria
- Tokio (async runtime), reqwest (HTTP), serde (serializacao)
- sqlx + SQLite (persistencia, async)
- clap (CLI), tracing (logs), thiserror (erros), anyhow (orquestracao)
- Configuracao via TOML + variaveis de ambiente

## Convencoes de codigo
- Erros de dominio com `thiserror` (`AppError`), retornados como `Result<T, AppError>`
- Erros de topo (main) com `anyhow`
- Todo I/O e async; nunca `block_on` fora do main (testes podem usar runtime manual)
- Logs estruturados com `tracing`, com campos nomeados (`url = %item.url`)
- Funcoes pequenas e testaveis
- Nao usar `unwrap()` em codigo de producao; so em testes
- Comentarios em portugues, codigo em ingles

## Fonte de dados: Querido Diario — FATOS VERIFICADOS
Spec: https://api.queridodiario.org.br/openapi.json (v0.19.0)
Detalhes em `docs/api-findings.md`. O que NAO fazer:

- NAO usar `queridodiario.ok.org.br/api/*` — responde 302. Use
  `https://api.queridodiario.org.br` (sem `/api`).
- NAO usar o parametro `keywords`. Ele e IGNORADO em silencio e a API devolve
  o pais inteiro com HTTP 200. O parametro correto e `querystring`.
- NAO usar aspas para montar frases no `querystring`: aspas exigem acento exato
  e podem retornar 0 em silencio. Use `+termo +termo` (AND) e sem acento.
- NAO usar `AND` textual: nao e honrado. So `+` funciona como AND.
- NAO assumir `published_at`, `external_id` ou `excerpt` (string). A resposta
  traz `date`, `excerpts` (array) e nenhum id — derive o id da `url`.
- NAO assumir que `size` passa de 200; pagine com `offset`.
- NAO assumir que a API e estavel: ha timeouts e 520 do Cloudflare.
  Todo request precisa de timeout >= 60s e retry com backoff.

## Regras de negocio
- Deduplicacao: `alerts.alert_hash` UNIQUE = sha256(source|external_id|keyword
  normalizada). `gazettes.id` = `<source>:<sha256(source|external_id)>`.
  A dedup e feita no banco com `INSERT OR IGNORE`, nunca "consulta antes de inserir".
- `matched_keyword` vem da fonte (uma consulta por keyword), NAO de busca no
  texto: o `querystring` combina varios termos (OR por padrao, `+` para exigir)
  e a API nao diz qual termo casou.
- Toda fonte implementa a trait `Source`. Fonte nova = modulo em `src/sources/`,
  implementa `Source`, registra em `src/sources/mod.rs`. Nada mais.
- `gazettes` + `alerts` sao gravadas na mesma transacao.

## Nao faca
- Nao inventar endpoints ou campos: se a API nao responder como esperado,
  pare e reporte com o corpo da resposta.
- Nao usar libs pesadas — o binario deve rodar em ~20 MB de RAM.
- Nao commitar segredos: token do Telegram so via env, nunca no config.toml.
- Nao implementar Telegram/dashboard/agendador antes do dia 3.
