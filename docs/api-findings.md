# Querido Diario — o que foi verificado na API real

Levantado em 2026-10-17 contra `https://api.queridodiario.org.br` (v0.19.0).
Spec oficial: <https://api.queridodiario.org.br/openapi.json>

Este documento existe para que ninguem (humano ou agente) volte a "chutar"
endpoints. Se um comportamento abaixo mudar, atualize aqui e no `AGENTS.md`.

## Host

| Tentativa | Resultado |
|---|---|
| `queridodiario.ok.org.br/api/cities` | **302** → `api.queridodiario.org.br/cities` |
| `queridodiario.ok.org.br/api/docs` | **302** → `api.queridodiario.org.br/docs` (**timeout**) |
| `api.queridodiario.ok.org.br` | handshake TLS falha |
| **`api.queridodiario.org.br`** | **200** — usar este |

O redirect do Cloudflare funciona se o cliente seguir 302, mas ir direto
economiza um round-trip e evita depender de redirect.

## Armadilha principal: `keywords` e ignorado

A API **nao tem** o parametro `keywords`. O filtro textual chama-se
`querystring` (sintaxe simple query string do OpenSearch).

Prova:

```
GET /gazettes?published_since=2025-10-10&published_until=2025-10-17&keywords=ZZZNAOEXISTE999
→ {"total_gazettes": 2332, ...}   # identico ao total sem filtro

GET /gazettes?published_since=2025-10-10&published_until=2025-10-17&querystring=licitacao
→ {"total_gazettes": 1502, ...}   # filtrou de verdade
```

Nenhum erro, nenhum aviso: HTTP 200 com o pais inteiro. Um monitor escrito com
`keywords` parece funcionar e grava tudo.

`querystring=licitação` e `querystring=licitacao` retornam o mesmo total
(1502) — a analise e insensivel a acento.

## Formato da resposta

`GET /gazettes` → `GazetteSearchResponse`:

```json
{
  "total_gazettes": 3,
  "gazettes": [
    {
      "territory_id": "4106902",
      "date": "2026-10-14",
      "scraped_at": "2026-10-16T02:47:19.047064",
      "url": "https://data.queridodiario.ok.org.br/4106902/2026-10-14/<sha1>.pdf",
      "territory_name": "Curitiba",
      "state_code": "PR",
      "excerpts": ["... texto ..."],
      "edition": "191",
      "is_extra_edition": false,
      "txt_url": "https://data.queridodiario.ok.org.br/4106902/2026-10-14/<sha1>.txt"
    }
  ]
}
```

Divergencias em relacao ao que o plano original assumia:

| Plano | Real |
|---|---|
| `published_at` | `date` |
| `excerpt: String` | `excerpts: [String]` |
| `external_id` | **nao existe** → derivar da `url` |
| — | `edition` pode vir `""`; `is_extra_edition` pode vir `null` |
| — | `scraped_at` e `txt_url` existem e sao uteis |

`excerpts` vem vazio quando `querystring` esta vazio ("an empty field returns
no excerpts, only the results metadata").

## Parametros uteis de /gazettes

`territory_ids` (repetivel, 7 digitos IBGE), `published_since`/`published_until`
(YYYY-MM-DD), `scraped_since`/`scraped_until`, `querystring`, `excerpt_size`
(default 500), `number_of_excerpts` (default 1), `pre_tags`/`post_tags`,
`size` (default 10), `offset`, `sort_by` = `relevance|descending_date|ascending_date`.

Verificado:

- `territory_ids` repetido soma corretamente: Curitiba(4) + Rio(3) = 7.
- `size=200` retorna 200 itens (nao ha teto de 50, apesar do default 10).
- Paginacao por `offset` funciona; janela profunda satura em 10000
  (Porto Alegre retorna `total_gazettes: 10000`, valor de corte).

## Cobertura por cidade (medido em 2026-10-17)

| Cidade | territory_id | total | ultima gazette |
|---|---|---|---|
| Curitiba | 4106902 | 9187 | 2026-10-15 |
| Rio de Janeiro | 3304557 | 6402 | 2026-10-15 |
| Salvador | 2927408 | 6346 | 2026-10-16 |
| Recife | 2611606 | 1857 | 2026-10-15 |
| Belo Horizonte | 3106200 | 4052 | 2026-03-25 |
| Porto Alegre | 4314902 | 10000 (corte) | 2025-10-31 |
| **Sao Paulo** | **3550308** | **20** | **2025-03-24** |
| Fortaleza | 2304400 | 0 | — |

Consequencia de projeto: **Sao Paulo nao e coberto**. O exemplo do plano
original (`territory_ids=3550308`) retorna zero — nao por bug no cliente.
Se o alvo for SP, a fonte precisa ser outra (DOC / Imprensa Oficial).

## Estabilidade

Durante o levantamento, cerca de um terco das requisicoes estourou 20 s e
apareceram respostas `error code: 520` (Cloudflare). Implicacoes:

- timeout >= 60 s (o cliente usa 90 s);
- retry com backoff exponencial em 5xx, 429 e timeout;
- sem concorrencia: uma requisicao por vez.

## Sintaxe do `querystring` (OpenSearch simple query string)

Medido em Curitiba, 2026-09-01..2026-10-17:

| querystring | total | leitura |
|---|---|---|
| `pregao` | 21 | termo solto, insensivel a acento |
| `eletronico` | 49 | — |
| `pregao eletronico` | 49 | termos sao **OR** por padrao |
| `pregao AND eletronico` | 49 | o `AND` **nao** foi honrado |
| `+pregao +eletronico` | 21 | **`+` funciona como AND** |
| `"pregao eletronico"` | **0** | frase exata: sem acento nao casa |
| `"pregão eletrônico"` | 21 | frase exata com acento casa |

Conclusoes que importam para o codigo e para a configuracao:

1. `AND` textual nao funciona; use `+` antes de cada termo obrigatorio.
2. Termo solto e insensivel a acento; **frase entre aspas nao e**. Um exemplo
   de config com `"pregao eletronico"` retornaria zero para sempre, sem erro —
   o mesmo tipo de falha silenciosa do `keywords`.
3. Por isso os exemplos de config usam `+termo +termo`, sem acentos e sem aspas.

## Ruido de alerta

`querystring=licitacao` casou com 1502 de 2332 diarios nacionais numa semana.
O motivo e visivel nos excerpts: muitos diarios tem uma secao "Licitações" no
sumario, que casa com o termo. Para reduzir ruido, exigir dois termos
(`+dispensa +licitacao`) em vez de um termo solto.

## Outros endpoints

- `GET /cities?city_name=...` → lista cidades; `level` 0-3 (0 = sem dados),
  `availability_date`, `publication_urls`.
- `GET /cities/{territory_id}` → uma cidade.
- `GET /gazettes/by_theme/themes/` → apenas 2 temas hoje
  ("Politicas Ambientais", "Tecnologias na Educacao") — inutil para licitacoes.
- Endpoints `/scraper/*` sao internos e exigem `X-API-Key`. Nao usar.
