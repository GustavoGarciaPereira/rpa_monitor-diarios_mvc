# Monitor de Diários

Monitor de diários oficiais: coleta publicações por palavra-chave, guarda em
SQLite sem duplicar e serve uma página web com filtros.

Rust + Tokio + axum + SQLite. Um binário, sem banco externo, sem SPA.

---

## O que faz

- **Coleta** publicações que casam com keywords em duas fontes: Querido Diário
  (nacional) e FAMURS (municípios do RS).
- **Deduplica** no banco: rodar o `scan` duas vezes seguidas reporta zero
  novidades, mesmo se o processo morrer no meio.
- **Publica** uma página em `http://127.0.0.1:3000` com estatísticas, filtros
  por cidade / palavra-chave / período e link para o PDF ou HTML original.

## Começando

```bash
git clone <repo> && cd Monitor_de_Diarios

cp config.toml.example config.toml    # config.toml é gitignored
# edite `keywords` e `territories`

cargo run -- scan --since 2026-06-01 --until 2026-10-17   # popula o banco
cargo run -- serve                                        # abre o site
```

Sem o `scan` a página abre vazia (mostra "Nenhuma publicação encontrada", não
um erro). Pare o servidor com `Ctrl+C`.

## Comandos

| Comando | O que faz |
|---|---|
| `monitor-diarios scan [--since YYYY-MM-DD] [--until YYYY-MM-DD]` | coleta e grava; default: últimos 7 dias |
| `monitor-diarios serve [--addr 127.0.0.1:3000]` | sobe a página e o `/health` |
| `monitor-diarios territories [--name "campinas"]` | lista cidades cobertas e seus IDs IBGE |

`--config <arquivo>` é global: `monitor-diarios --config outro.toml serve`.

## Configuração

`config.toml` (veja [`config.toml.example`](config.toml.example)):

| Campo | Default | Observação |
|---|---|---|
| `keywords` | — | obrigatório para `scan` |
| `territories` | `[]` | IDs IBGE de 7 dígitos; vazio = busca nacional |
| `famurs_entities` | `[]` | IDs de entidade do portal da FAMURS (RS) |
| `processos` | `[]` | processos do DataJud (`numero`, `tribunal`, `apelido`) |
| `datajud_api_key` | — | chave pública do DataJud (também em `DATAJUD_API_KEY`) |
| `database_url` | `sqlite:monitor.db` | |
| `page_size` | `200` | itens por página na API |
| `max_results_per_keyword` | `2000` | teto por keyword, evita varrer o país |
| `telegram_token` / `telegram_chat_id` | — | reservados; hoje só logam |

Variáveis de ambiente sobrescrevem o arquivo:

```
MONITOR_CONFIG        caminho do TOML
DATABASE_URL          url do SQLite
MONITOR_KEYWORDS      lista separada por vírgula
MONITOR_TERRITORIES   lista separada por vírgula
MONITOR_FAMURS_ENTITIES
MONITOR_TEMPLATE_FILE template lido do disco a cada request (dev)
TELEGRAM_TOKEN / TELEGRAM_CHAT_ID
```

Segredos vão **só** em env — nunca no `config.toml`, que é versionável.

### ⚠️ Sintaxe das keywords

As keywords são escritas na sintaxe do `querystring` do Querido Diário:

```toml
keywords = ["+pregao +eletronico", "+dispensa +licitacao"]
```

- `+termo` exige o termo (AND). Sem `+`, os termos são **OR** e o resultado
  vira ruído.
- **Não use acentos nem aspas.** Termo solto é insensível a acento; frase entre
  aspas é match exato e costuma voltar **zero** sem erro.
- `AND` textual não funciona. Só `+`.

Essas armadilhas estão documentadas com medições em
[`docs/api-findings.md`](docs/api-findings.md).

## Fontes de dados

| Fonte | Cobertura | Formato |
|---|---|---|
| `querido_diario` | nacional, por município | JSON (API) |
| `famurs` | municípios do RS conveniados | HTML |
| `datajud` | processos específicos (CNJ) | JSON (Elasticsearch) |

**A cobertura é desigual e isso muda o resultado.** Exemplos verificados:

- São Paulo (3550308) tem 20 publicações, a última de 2025-03-24.
- O RS está estagnado na fonte nacional; várias cidades retornam zero.
- Santa Maria (RS) não tem fonte acessível (site sem acervo).

Antes de confiar numa cidade, cheque o volume real — detalhes e números em
[`docs/api-findings.md`](docs/api-findings.md) e
[`docs/fontes-rs.md`](docs/fontes-rs.md).

Adicionar uma fonte nova = criar um módulo em `src/sources/`, implementar a
trait `Source` e registrar em `src/sources/mod.rs`.

## Como a deduplicação funciona

```
gazettes.id   = "<source>:<sha256(source|external_id)>"
alerts.alert_hash = sha256(source|external_id|keyword normalizada)
```

O banco é a fonte da verdade: `INSERT OR IGNORE` + `rows_affected`, nunca
"consulta antes de inserir". `gazettes` e `alerts` entram na mesma transação,
então uma falha no meio não deixa publicação órfã.

Uma publicação que casa com 2 keywords gera **2 alertas** (é o mesmo `id` de
gazette), o que permite medir cada keyword separadamente.

## Página web

`cargo run -- serve` → <http://127.0.0.1:3000>

- `/` — cards de estatísticas, filtros (`city`, `keyword`, `days`) e lista
- `/health` — responde `ok`, usado pelo health check do deploy

O template é um HTML único (`templates/index.html`) embutido no binário via
`include_str!` e estilizado com Tailwind por CDN.

Em build de **debug** (dev), o HTML é lido do disco a cada request: edite
`templates/index.html` e recarregue o browser — sem recompilar. O release usa o
embutido (zero I/O por request); `MONITOR_TEMPLATE_FILE=/caminho.html` força a
leitura de um arquivo no disco.

## Deploy

Imagem Docker multi-stage (build em `rust:1.92-slim`, runtime
`debian:bookworm-slim`, ~90 MB):

```bash
docker build -t monitor-diarios .
docker run --rm -p 8080:8080 -v "$(pwd)/data:/app/data" monitor-diarios
curl localhost:8080/health
```

No Fly.io:

```bash
fly apps create <seu-app>          # ajuste `app` no fly.toml
fly volumes create monitor_data --region gru --size 1
fly deploy
```

O `start.sh` roda um `scan` de 30 dias antes de subir o servidor. Variáveis de
produção ficam no `[env]` do [`fly.toml`](fly.toml) (não há `config.toml` na
imagem).

## Desenvolvimento

```bash
cargo test                                  # 35 testes
cargo clippy --all-targets -- -D warnings   # precisa passar limpo
cargo build --release

# Loop de dev: reinicia o servidor sozinho a cada arquivo salvo
cargo install cargo-watch                   # uma vez
cargo watch -x 'run -- serve'
```

```
src/
├── main.rs          CLI (clap) e dispatch
├── config.rs        TOML + env, validação por comando
├── db.rs            pool SQLite + migrações
├── error.rs         AppError (thiserror)
├── http.rs          GET com retry e backoff
├── model.rs         RawGazette, hashes, normalização de texto
├── sources/         trait Source + querido_diario + famurs
├── commands/        scan, serve, territories
migrations/          schema
templates/           index.html
docs/                achados verificados das fontes
```

O [`AGENTS.md`](AGENTS.md) traz as convenções e as armadilhas já descobertas —
útil para qualquer agente ou pessoa que for mexer no código.

## Limitações conhecidas

- **Cobertura da fonte**: cidades podem simplesmente não ter dados (ver acima).
- **Sem agendador interno**: rode `scan` por cron/systemd timer.
- **Telegram não implementado**: os campos existem, mas só há log.
- **Cold start lento**: `auto_stop_machines` + scan no boot faz a primeira
  visita demorar; mitigado com `grace_period` de 2 min no health check.
- **Um excerpt por publicação**: se duas keywords casam, o trecho guardado
  corresponde a uma delas.
- **`territory_id` da FAMURS** é o ID da entidade no SIGPub, não o IBGE.
