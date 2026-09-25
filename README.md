A command-line research assistant for literature searches. You describe a topic in
plain language. The agent turns it into database queries, searches **PubMed** and
**bioRxiv**, downloads the papers it finds, and may write a summary if you choose into 
a folder of your choice.
The LLM decides which searches to run and what to write. Everything, from the queries, 
downloads, files goes through five tools defined. 

e.g.
```
>  5 most recent papers on new metagenonomics assemblers, bioRxiv

  searching bioRxiv: metagenome assembler OR metagenomic assembly algorithm
  downloading 10.1101/2025.09.05.674543 from bioRxiv
  downloading 10.1101/2024.08.09.607291 from bioRxiv
  writing summary.md

Saved 5 preprints and summary.md to /data/projects/assembly_review.
Two PDFs were refused by bioRxiv's rate limiter; try those again in a few minutes.
```

## What it does

| Tool | What it does |
|---|---|
| `search_pubmed` | Searches PubMed for papers with free full text, published after a given year. Returns PMID, PMC id, DOI and title, one line each. |
| `download_pubmed` | Downloads PMC open-access papers: the PDF plus a markdown version of the full text, converted from PMC's structured XML. |
| `search_biorxiv` | Searches bioRxiv preprints through Europe PMC. Returns DOI, year and title. |
| `download_biorxiv` | Downloads a preprint's PDF from biorxiv.org and extracts its text alongside it. |
| `save_report` | Writes the model's summary to a file, one section per call, so long reports are not truncated. |

Papers come back as **PDF + markdown** from PubMed Central and **PDF + plain text**
from bioRxiv. Preprint PDFs carry no structure worth converting, so their text is
extracted as-is.

## Requirements

- Rust 1.98 or newer (edition 2024)
- An OpenAI API key

## Installation  

To install Rust and Cargo follow the instructions in https://doc.rust-lang.org/cargo/getting-started/installation.html  

Download the repo.  
```bash
git clone <this repository>
cd lit_agent
cargo build --release
```

## Usage

```bash
export OPENAI_API_KEY='...'
cargo run --release
```

Then talk to it. It will ask what you want to search for, which database to use,
how many papers, from which year, and where to save them. Type `exit` to leave.

Give it an **absolute path** for the output folder. The folder is created if it does not exist.


## Limitations

- PubMed downloads Papers with a PMC id. They might not be downloadable.
- Text extracted from bioRxiv PDFs keeps the line numbers printed in the margin and
  can lose some ligatures. It is meant for machine reading, not for printing.
- The search quality depends entirely on the query the model writes. Read the
  queries it reports.

## Built with

[rig](https://github.com/0xPlaygrounds/rig) ·
[pubmed-client](https://crates.io/crates/pubmed-client) ·
[pdf-extract](https://crates.io/crates/pdf-extract) ·
[reqwest](https://crates.io/crates/reqwest) ·
[tokio](https://tokio.rs)

Literature data comes from [PubMed](https://pubmed.ncbi.nlm.nih.gov/),
[PubMed Central](https://www.ncbi.nlm.nih.gov/pmc/),
and [bioRxiv](https://www.biorxiv.org/).
