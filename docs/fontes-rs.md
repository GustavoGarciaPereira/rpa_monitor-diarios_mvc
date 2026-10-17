# Fontes para o Rio Grande do Sul

Levantado em **2026-10-17**. Motivo: Santa Maria (4316907) e Sao Sepe (4319604)
retornam zero publicacoes no Querido Diario — ver `api-findings.md`.

Este documento registra onde cada municipio publica de fato, o que da para
raspar e o que nao da. Se um comportamento abaixo mudar, atualize aqui.

| Cidade | territory_id (IBGE) | Fonte | Situacao |
|---|---|---|---|
| Sao Sepe | 4319604 | FAMURS / SIGPub | **viavel** |
| Santa Maria | 4316907 | site proprio (Livewire) | **sem fonte acessivel** |

---

## Sao Sepe — FAMURS (viavel)

**Portal:** <https://www.diariomunicipal.com.br/famurs/>
Diario Oficial dos Municipios do Estado do Rio Grande do Sul (SIGPub).

**Entidade:** `64462` — *Prefeitura Municipal de Sao Sepe*.
O portal tem 304 entidades (municipios, camaras, consorcios, agencias). O mesmo
scraper serve qualquer uma delas trocando esse id.

### Listagem — GET, HTML renderizado no servidor

```
https://www.diariomunicipal.com.br/famurs/pesquisar
  ?busca_avancada[entidade]=64462
  &busca_avancada[dataInicio]=2026-09-01
  &busca_avancada[dataFim]=2026-10-17
  &busca_avancada[ordenacao]=recente
  &busca_avancada[pagina]=1
```

Parametros aceitos pelo formulario:

| Parametro | Uso |
|---|---|
| `busca_avancada[texto]` | palavra-chave no texto |
| `busca_avancada[titulo]` | titulo da materia |
| `busca_avancada[entidade]` | id do municipio/orgao (obrigatorio na pratica) |
| `busca_avancada[orgao]` | secretaria |
| `busca_avancada[dataInicio]` / `[dataFim]` | `YYYY-MM-DD` |
| `busca_avancada[ordenacao]` | `recente` (usado) |
| `busca_avancada[pagina]` | paginacao, 1-based |

Nao precisa de JavaScript nem de sessao: `curl` devolve o HTML completo.

### Estrutura do HTML

```html
<p class="busca-avancada__resumo">
  Mostrando <strong>1</strong> a <strong>25</strong> de <strong>94</strong> materias.
</p>

<ul class="lista-materias">
  <li class="materia-card">
    <h2 class="materia-card__titulo">
      <a href="/famurs/materia/D6898EDE">RETIFICAÇÃO DE EDITAL</a>
    </h2>
    <p class="materia-card__origem">
      <span class="materia-card__entidade">Prefeitura Municipal de São Sepé</span>
      <span class="materia-card__separador">•</span>
      <span class="materia-card__orgao">Secretaria Municipal de Administração</span>
    </p>
    <dl class="materia-card__dados">
      <div><dt>Circulação</dt><dd>13/10/2026</dd></div>
      <div><dt>Edição</dt><dd>4434</dd></div>
      <div><dt>Código identificador</dt><dd><code>D6898EDE</code></dd></div>
    </dl>
    <p class="materia-card__acoes">
      <a class="acao acao--principal" href="/famurs/materia/D6898EDE">Abrir matéria</a>
      <a class="acao" href="/famurs/download/materia/D6898EDE">Baixar matéria (HTML)</a>
      <a class="acao" href="/famurs/download/edicao/D6898EDE">Baixar edição (PDF)</a>
    </p>
  </li>
</ul>
```

- Seletor da lista: `ul.lista-materias li.materia-card`
- Titulo: `h2.materia-card__titulo > a`
- Data de circulacao: `dl.materia-card__dados` → `<dt>Circulação</dt><dd>DD/MM/YYYY</dd>`
- Codigo: `<dd><code>XXXXXXXX</code></dd>`

### Conteudo — texto completo em HTML

```
https://www.diariomunicipal.com.br/famurs/download/materia/D6898EDE
```

Responde `200 text/html` (~35 KB) com o texto integral da publicacao, ja em
paragrafo corrido. **Nao e preciso baixar PDF.**

Exemplo real do conteudo:

> ESTADO DO RIO GRANDE DO SUL PREFEITURA MUNICIPAL DE SÃO SEPÉ ... RETIFICAÇÃO
> DE EDITAL Ref. Edital de Pregão Eletrônico nº 50/2026 ... TORNA PÚBLICO ...
> ALTERAÇÃO DO EDITAL de licitação ...

### O PDF da edicao inteira (nao usar por padrao)

```
https://www.diariomunicipal.com.br/famurs/download/edicao/D6898EDE
```

`200 application/pdf`, ~3,7 MB. Cada materia aponta para a edicao do dia, que
traz atos de varios municipios. So vale a pena se algum dia precisarmos do
documento original — para extrair texto, o endpoint de materia e melhor.

### `external_id`

O **codigo identificador da materia** (`D6898EDE`, 8 hex). Estavel, unico e
presente tanto na listagem quanto na URL de download. Use ele direto como
`external_id` — nao precisa de hash de URL como no Querido Diario.

### Permissao

`https://www.diariomunicipal.com.br/robots.txt`:

```
User-agent: *
Disallow:
```

Sem restricao. Ainda assim: sem concorrencia e com pausa entre paginas.

### Volume medido (entidade 64462)

| Recorte | Materias |
|---|---|
| 12 meses (2025-10 a 2026-10) | 144 |
| 2026 inteiro com `texto=licitação` | 94 |
| Por pagina | 25 |

### Riscos conhecidos

- O HTML pode mudar de classes sem aviso (scraper acoplado a `materia-card`).
- Um municipio pode sair do consorcio; nesse caso a entidade some do select
  (304 entidades hoje) e a busca retorna vazio sem erro.
- Filtrar por `texto` no servidor reduz muito o volume — preferir isso a baixar
  tudo e filtrar localmente.

---

## Santa Maria — sem fonte acessivel hoje

**Portal oficial:** <https://www.santamaria.rs.gov.br/diario-oficial>
Laravel + **Livewire**. Verificado em **2026-10-17**.

O sistema responde `200` e tem UI completa (calendario, busca por periodo e por
palavra-chave), mas **nao entrega acervo**:

| Verificacao | Resultado |
|---|---|
| `<script id="do-calendar-data">` (fonte do calendario) | `[]` (vazio) |
| `/diario-oficial/textos?data_inicial=&data_final=` | "Nenhum resultado encontrado" |
| Mesma rota com 2024 inteiro | vazio |
| Mesma rota com `texto=licitacao` | vazio |
| Requisicoes XHR ao abrir a pagina (Chrome real) | **nenhuma** — nao existe API |
| Links `.pdf` na pagina | zero |
| Subdomínios `diariooficial.`, `doe.`, `imprensa.`, `portal.` | DNS inexistente |
| `office.santamaria.rs.gov.br/diario-oficial` | espelho da mesma pagina vazia |
| Snapshots no Wayback Machine | nenhum |

Tambem foi testado carregar a pagina em Chrome headless com JavaScript
habilitado: o calendario renderiza "Outubro de 2026" com todos os dias
desabilitados. Nao e bloqueio de bot nem carregamento tardio — o servidor
realmente devolve vazio.

### Por que nao da para raspar

Nao ha PDF, JSON ou endpoint publico. O dado que alimenta o calendario viria
embutido no HTML (`do-calendar-data`), e vem vazio. Sem esse payload, nao ha o
que coletar.

### Como reavaliar

1. Contatar a Prefeitura de Santa Maria (setor de publicacoes / STI) e perguntar
   onde o Diario Oficial e publicado oficialmente.
2. Re-testar `do-calendar-data` a cada ~30 dias — a pagina parece recem-migrada.
3. Se surgir portal terceirizado, adicionar como nova entidade/secao aqui.

---

## Implicacao para a implementacao

1. Criar a fonte da **FAMURS** parametrizada por `entidade`, nao uma fonte
   `rs_municipal` especifica: as outras 303 entidades passam a funcionar
   trocando um ID na configuracao.
2. Santa Maria nao entra na lista de territorios ativos. Registrar no log da
   coleta que a cidade nao tem fonte, para nao parecer "monitorando e sem
   novidades".
3. `scraper` e suficiente. `pdf-extract` so se um dia precisarmos das edicoes
   completas — o endpoint de materia entrega texto puro em HTML.
