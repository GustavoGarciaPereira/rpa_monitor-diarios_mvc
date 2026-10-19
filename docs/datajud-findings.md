# DataJud (CNJ) — o que foi verificado na API pública

Levantado em **2026-10-19** contra `https://api-publica.datajud.cnj.jus.br`.
Wiki oficial: <https://datajud-wiki.cnj.jus.br/api-publica/acesso/>

Este documento registra só o que foi testado na prática. Onde algo não foi
verificado, está dito explicitamente.

---

## 1. Base URL e índices

Base: `https://api-publica.datajud.cnj.jus.br`

Cada tribunal é um índice Elasticsearch no padrão `api_publica_<tribunal>`
(minúsculo). **Não há como listar os índices**: `_cat/indices` e
`_cluster/health` respondem `403` com a chave pública:

```json
{"error":{"root_cause":[{"type":"security_exception",
 "reason":"action [indices:monitor/settings/get] is unauthorized for API key
 id [p6Gc9YkBZuTDd2BzwPmv] of user [dpj_api_publica]"}]}}
```

A chave pertence ao usuário `dpj_api_publica` e só tem `_search`. Os índices
abaixo foram confirmados um a um.

| Índice | Resultado |
|---|---|
| `api_publica_tjsp` | **200** |
| `api_publica_stj` | **200** |
| `api_publica_tjrs` | **200** |
| `api_publica_trf4` | **200** |
| `api_publica_stf` | **404** — `index_not_found_exception` |

**O STF não está no DataJud.** Qualquer requisito de "acompanhar processo no
STF" não é atendido por esta fonte.

## 2. Autenticação

A chave é pública e divulgada na própria wiki. Vigente em **2026-10-19**:

```
APIKey cDZHYzlZa0JadVREZDJCendQbXY6SkJlTzNjLV9TRENyQk1RdnFKZGRQdw==
```

Header obrigatório em toda requisição:

```
Authorization: APIKey <chave>
Content-Type: application/json
```

O CNJ avisa na wiki que **pode trocar a chave a qualquer momento** — vale ler a
chave da página em vez de assumir a hardcoded.

Erros de autenticação (ambos `401`):

```json
{"error":{"root_cause":[{"type":"security_exception",
 "reason":"missing authentication credentials for REST request [...]","status":401}]}}
```

```json
{"error":{"root_cause":[{"type":"security_exception",
 "reason":"unable to authenticate with provided credentials and anonymous access is not allowed",
 "additional_unsuccessful_credentials":"API key: Illegal base64 character 2d","status":401}]}}
```

## 3. Busca

```
POST /{indice}/_search
```

Body no formato Elasticsearch (query DSL). Por número exato:

```bash
curl -X POST "https://api-publica.datajud.cnj.jus.br/api_publica_tjsp/_search" \
  -H "Authorization: APIKey $KEY" \
  -H "Content-Type: application/json" \
  -d '{"size":1,"query":{"match":{"numeroProcesso":"10047296420238260481"}}}'
```

Por intervalo de atualização (o caso interessante para monitoramento):

```json
{
  "size": 0,
  "track_total_hits": true,
  "query": {
    "range": { "dataHoraUltimaAtualizacao": { "gte": "2026-10-01T00:00:00.000Z" } }
  }
}
```

`size: 0` devolve só metadados (`took`, `hits.total`) — é assim que se conta
sem transferir documentos.

## 4. Resposta real

Requisição acima (busca pelo número exato). Resposta **real**, com o array
`movimentos` truncado — ele veio com 83 itens:

```json
{
  "took": 33828,
  "_shards": { "total": 20, "successful": 20, "skipped": 0, "failed": 0 },
  "hits": {
    "total": { "value": 1, "relation": "eq" },
    "max_score": 14.164036,
    "hits": [
      {
        "_index": "api_publica_tjsp",
        "_id": "TJSP_G1_10047296420238260481",
        "_score": 14.164036,
        "_source": {
          "id": "TJSP_G1_10047296420238260481",
          "tribunal": "TJSP",
          "grau": "G1",
          "numeroProcesso": "10047296420238260481",
          "dataAjuizamento": "20231025114738",
          "nivelSigilo": 0,
          "orgaoJulgador": {
            "codigo": 10121,
            "nome": "Juízo Titular I - 2ª Vara da Comarca de Presidente Epitácio",
            "codigoMunicipioIBGE": 3541307
          },
          "classe": { "codigo": 7, "nome": "Procedimento Comum Cível" },
          "sistema": { "codigo": 4, "nome": "Projudi" },
          "formato": { "codigo": 1, "nome": "Eletrônico" },
          "dataHoraUltimaAtualizacao": "2026-10-06T22:21:09.263000Z",
          "@timestamp": "2026-10-06T22:21:09.263000Z",
          "assuntos": [ { "codigo": 10588, "nome": "Vícios de Construção" } ],
          "movimentos": [
            {
              "codigo": 123,
              "dataHora": "2026-10-01T07:34:29.000Z",
              "nome": "Remessa",
              "complementosTabelados": [
                {
                  "codigo": 18,
                  "descricao": "motivo_da_remessa",
                  "valor": 38,
                  "nome": "em grau de recurso"
                }
              ],
              "orgaoJulgador": { "codigo": "10121", "nome": "Juízo Titular I - ..." }
            }
          ]
        }
      }
    ]
  }
}
```

## 5. Campos

| Campo | Tipo observado | Observação |
|---|---|---|
| `id` | string | `TRIBUNAL_GRAU_NUMERO` — **bom candidato a `external_id`** |
| `tribunal` | string | `TJSP`, `STJ`, `TJRS`… |
| `grau` | string | `G1`, `G2`, `JE`, `SUP` |
| `numeroProcesso` | string | 20 dígitos, sem pontuação |
| `dataAjuizamento` | string | `YYYYMMDDHHMMSS`; muitas vezes só a data, com hora `000000` |
| `dataHoraUltimaAtualizacao` | ISO 8601 UTC | igual ao `@timestamp` |
| `nivelSigilo` | int | `0` = público |
| `orgaoJulgador` | objeto | `codigo`, `nome`, `codigoMunicipioIBGE` |
| `classe` | objeto | `codigo`, `nome` |
| `assuntos` | array | `codigo`, `nome` |
| `movimentos` | array | `codigo`, `nome`, `dataHora`, `complementosTabelados`, `orgaoJulgador` |
| `sistema` / `formato` | objeto | ex.: Projudi / Eletrônico |

`_id` do documento e `_source.id` são iguais.

## 6. Número do processo

Formato CNJ `NNNNNNN-DD.AAAA.J.TR.OOOO` (20 dígitos). Na API vai **sem
pontuação**.

Exemplo real (TJSP): `10047296420238260481`

```
1004729 - 64 . 2023 . 8 . 26 . 0481
   |       |     |     |   |     |
sequencial DV  ano  justiça TR  comarca
```

- `J=8` Justiça Estadual, `TR=26` São Paulo, `OOOO=0481` Presidente Epitácio.
- `00011552020248272734` (STJ) → `0001155-20.2024.8.27.2734`: origem no RS.
- `50006840620168210166` (TJRS) → `5000684-06.2016.8.21.0166`.

Ao exibir, converter para o formato pontuado; ao consultar, usar os 20 dígitos.

## 7. Atualização incremental

`dataHoraUltimaAtualizacao` é indexado e aceita `range` — é a base para
monitorar "o que mudou desde ontem".

Medido no **TJRS** (índice inteiro: **14.385.741** processos):

| Filtro | Resultado | Tempo |
|---|---|---|
| total do índice | 14.385.741 | 9,2 s |
| `>= 2026-09-01` | **2.757.907** | 5,7 s |
| `>= 2026-01-01` | **9.829.360** | 12,9 s |

Ou seja: **~2,7 milhões de processos atualizados em ~7 semanas só no TJRS.**
Varrer "tudo que mudou" não é um monitor, é um pipeline de dados. Uso realista:
acompanhar **processos específicos** (por número) ou recortar por
`orgaoJulgador` / `classe` / `assuntos`.

## 8. Latência, rate limit e limites

Tempos medidos (rede + servidor):

| Requisição | Tempo |
|---|---|
| TJRS `size:0` | 8,9 s |
| TJRS busca por número (0 hits) | 24,3 s |
| TJRS intervalo | 5,7 s |
| TJSP `size:0` | 27,3 s |
| TJSP busca por número exato | 34,2 s |
| STJ amostra (`size:1`) | 17,1 s |

O `took` do Elasticsearch (33.828 ms) é praticamente o tempo total: a lentidão
é **do lado do servidor**, não da rede.

- **Sem headers de rate limit**, mas **existe backpressure**. 5 requisicoes
  simultaneas voltaram `200` (17,9 s a 33,1 s) e nenhum header `X-Rate-Limit`,
  `RateLimit-*` ou `Retry-After` apareceu. Porem, sob carga, a API devolve
  **`429` com `es_rejected_execution_exception`** — o thread pool do
  Elasticsearch recusou a busca. Nao ha header dizendo quando tentar de novo:
  trate `429` como transitorio e reconsulte mais tarde.
- Ainda assim: use **timeout ≥ 120 s** e requisições seriais. O serviço é
  público e compartilhado.
- **Janela de resultados:** `from + size <= 10000`. Acima disso:
  `illegal_argument_exception: Result window is too large`. Para volume, usar
  `search_after`, não `from`.
- Sem `track_total_hits: true`, `hits.total.value` satura em `10000` com
  `relation: "gte"`.

## 9. Qualidade dos dados

- **Ordem de `movimentos` não é garantida.** No TJSP e TJRS veio decrescente
  (mais recente primeiro); no STJ veio **crescente**. Nunca assumir
  `movimentos[0]` — ordenar por `dataHora`.
- **Encoding quebrado em pelo menos um registro do STJ:** `orgaoJulgador.nome`
  veio como a sequencia `PRESID` + caractere de controle + `NCIA`, isto e,
  "PRESIDENCIA" gravada em UTF-8 e lida como Latin-1 (mojibake). Afeta
  exibicao, nao a busca.
- `dataAjuizamento` às vezes tem hora zerada (`20260828000000`).
- `nivelSigilo != 0` indica processo sigiloso; o conteúdo pode vir reduzido.

## 10. Implicações para implementação

1. **Consulta por número de processo é viável** (~20–35 s por consulta). É o
   caso de uso que faz sentido.
2. **"Tudo que mudou" não é viável** sem recorte: 2,7 mi de atualizações em
   7 semanas só no TJRS.
3. `external_id` natural = `_source.id` (`TJSP_G1_<numero>`), único por índice.
4. Não existe push/callback: é polling. Definir intervalo pensando em 30 s por
   requisição.
5. Timeout alto e retry; nenhuma indicação de rate limit, mas serial.
6. Não há STF — se o requisito incluir STF, é outra fonte (API própria do STF).
7. Se um dia entrar como `Source`, o `territory_id` não é IBGE: usar `tribunal`
   (ex.: `TJSP`) e `grau` como parte da identidade.

---

## 11. Instabilidade observada em campo (2026-10-19)

Durante a implementacao, no mesmo dia do levantamento, a API piorou:

- `api_publica_tjsp` passou a responder **`504 Gateway Time-out` (NSX LB) em
  ~60 s** para a consulta que antes levava 34 s. O corte e do load balancer,
  nao do cliente: nosso timeout de 120 s nao ajuda.
- **A mesma consulta por numero retornou `total: 0` numa execucao e `total: 1`
  na seguinte**, sem erro HTTP. Consequencia: **`0 hits` nao prova que o
  processo nao existe.** Um monitor precisa reconsultar; nao pode concluir
  "processo encerrado/inexistente" a partir de uma resposta vazia.
- O Elasticsearch pode devolver **`200` com `"timed_out": true`** e zero hits.
  Tratar isso como "nao encontrado" gera falso negativo silencioso — o codigo
  em `src/sources/datajud.rs` devolve erro nesse caso.

Medido no mesmo periodo: STJ ~49 s por consulta exata, TJSP 34-60 s (com 504
no limite), TJRS 6-13 s para contagem por intervalo.

- Apareceram tambem **`429 es_rejected_execution_exception`** (busca recusada
  pelo thread pool) e respostas vazias sem erro, intercaladas com sucessos.

Ainda assim, uma execucao real completou: 7 movimentos do processo
`00011552020248272734` (STJ) foram coletados e gravados; a execucao seguinte
recoletou os mesmos 7 e gravou **0** (dedup por `external_id` de movimento).
