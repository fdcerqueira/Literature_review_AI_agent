A command-line research assistant for literature searches. The user describes a topic.   
The agent turns it into database queries, searches **PubMed** and
**bioRxiv**, optionally screens the results for relevance before download, downloads the papers, and if
asked reads them and writes a summary report into tho chosen folder.

Two of those tools employ their own **subagents**: one screens search results against
what the user asked for, the other reads each downloaded paper. Both keep the text they
work on out of the main conversation, and both run on a cheaper model than the conversation does.

## Tools

| Tool | What it does |
|---|---|
| `search_pubmed` | Searches PubMed for papers with free full text, published after a given year. Returns PMID, PMC id, DOI, title and abstract. With screening on, a subagent reads those abstracts and only the matching papers come back. |
| `search_biorxiv` | The same for bioRxiv preprints, through Europe PMC, returning DOI, year, title and abstract. |
| `download_pubmed` | Downloads PMC open-access papers: the PDF plus a markdown version of the full text, converted from PMC's structured XML. |
| `download_biorxiv` | Downloads a preprint's PDF from biorxiv.org and extracts its text alongside it. |
| `create_summary` | Reads every downloaded paper in a folder, one subagent call per paper, and returns a short summary of each. |
| `save_report` | Writes the report to a file. |

Papers come back as **PDF + markdown** from PubMed Central and **PDF + plain text**
from bioRxiv. 

Everything PMC gives alongside an article: figures, supplementary tables, its XML
and JSON, goes into an `other_files/` subfolder. When
screening is used, the papers it dropped are listed in `out_of_scope_papers.md` with
their identifiers and the reason each was excluded, so nothing disappears silently.

## Providers

The provider is whichever API key is set. Exactly one, or it refuses to start. Each provider names two models: the conversation runs on the capable one,
the two subagents on the cheaper one.

| Key | Conversation | Screener + summarizer |
|---|---|---|
| `ANTHROPIC_API_KEY` | claude-opus-5-5 | claude-haiku-4-5 |
| `OPENAI_API_KEY` | gpt-5.6 | gpt-5-mini |
| `MOONSHOT_API_KEY` | kimi-k2.6 | kimi-k2.5 |
| `DEEPSEEK_API_KEY` | deepseek-v4-pro | deepseek-flash |

Both are one line each to change, in `Provider::model()` and
`Provider::worker_model()`. To use different models, edit two functions in `src/main.rs`: **`model()` at line 41**
picks the conversation model, **`worker_model()` at line 51** picks the one the
screener and the summarizer run on. One line per provider in each, then
`cargo build --release`. 

## What it costs

Screening and reading are optional and the agent asks before either. Reading is by
far the expensive one. Rough figures from a five-paper run:

| Step | Relative | On which model |
|---|---|---|
| Search and download | **1×** | conversation model |
| Screening 10 candidates | **0.25×** | worker model |
| Reading 5 papers | **2×–8×** | worker model |
| Writing the report | **~1×** | conversation model |

Reading requires more  tokens, despite the fact it runs on subagent, which is a fraction of the price. 

- With `show_usage` on, the per-turn token counts are the **main agent's only**.
  
## Requirements

- Rust 1.98 or newer (edition 2024)
- An API key for one of the four providers above

## Installation

To install Rust and Cargo, follow the instructions at
https://doc.rust-lang.org/cargo/getting-started/installation.html

```bash
git clone <this repository>
cd lit_agent
```

## Usage

```bash
export ANTHROPIC_API_KEY='...'      # or OPENAI_, MOONSHOT_, DEEPSEEK_
cargo run --release
```

`cargo run` builds first if anything changed, so the first run takes a few minutes
while the dependencies compile and later ones start immediately.    


It prints which provider and models it is using, then asks what you want to search
for, which database, how many papers, from which year, where to save them, whether to
screen the results, and whether to read the papers and write a report. Type `exit` to
leave.

Give it an **absolute path** for the output folder. The folder is created if it does
not exist.

## Limitations

- PubMed downloads cover the PMC **open-access subset**. A paper with a PMC id is not
  necessarily downloadable.
- Each paper is truncated at 150,000 characters before it reaches the summarizer.
  Longer papers lose their final sections, and the summary says so when it happens.
- bioRxiv's rate limit is shared across runs from the same address. Several sessions
  in a few minutes will hit it even with the pause is set to.
- Search quality depends entirely on the query the model writes. Read the queries it
  reports.

## References

[rig](https://github.com/0xPlaygrounds/rig) ·
[pubmed-client](https://crates.io/crates/pubmed-client) ·
[pdf-extract](https://crates.io/crates/pdf-extract) ·
[reqwest](https://crates.io/crates/reqwest) ·
[reqwest-retry](https://crates.io/crates/reqwest-retry) ·
[tokio](https://tokio.rs)

Literature data comes from [PubMed](https://pubmed.ncbi.nlm.nih.gov/),
[PubMed Central](https://www.ncbi.nlm.nih.gov/pmc/),
[Europe PMC](https://europepmc.org/) and [bioRxiv](https://www.biorxiv.org/).
Please respect their terms of use and rate limits.
