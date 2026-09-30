use rig::agent::{Agent, AgentBuilder};
use rig::integrations::cli_chatbot::ChatBotBuilder;
use rig::prelude::*;
use rig::providers::anthropic::completion::ANTHROPIC_VERSION_LATEST;
use rig::providers::{anthropic, deepseek, moonshot, openai};
use reqwest_middleware::ClientWithMiddleware;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::RetryTransientMiddleware;
use reqwest_retry::policies::ExponentialBackoff;
use std::env;
use std::time::Duration;

mod tools;
use tools::{SearchPubMed, DownloadPubMed, SearchBiorXvir, DownloadBiorXvir, SaveReport, CreateSummary};

// ---------------------------provider-------------------------------------

#[derive(Clone, Copy, Debug)]
enum Provider {
    Anthropic,
    OpenAi,
    Moonshot,
    DeepSeek,
}

impl Provider {
    const ALL: [Provider; 4] =[Provider::Anthropic,
                                Provider::OpenAi,
                                Provider::Moonshot,
                                Provider::DeepSeek];

    fn env_var(&self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAi => "OPENAI_API_KEY",
            Provider::Moonshot => "MOONSHOT_API_KEY",
            Provider::DeepSeek=> "DEEPSEEK_API_KEY",
        }
    }

    fn model(&self) -> &'static str {
        match self {
            Provider::Anthropic => "claude-opus-5-5",
            Provider::OpenAi=>openai::completion::GPT_5_6,
            Provider::Moonshot=> "kimi-k2.6",
            Provider::DeepSeek => deepseek::DEEPSEEK_V4_PRO,
        }
    }

    //the screener and the summarizer cheaper models
    fn worker_model(&self) ->&'static str {
        match self {
            Provider::Anthropic => anthropic::completion::CLAUDE_HAIKU_4_5,
            Provider::OpenAi => openai::completion::GPT_5_MINI,
            Provider::Moonshot => moonshot::KIMI_K2_5,
            Provider::DeepSeek =>"deepseek-flash",
        }
    }
}

fn detect_provider()->Result<(Provider,String), anyhow::Error> {
    let found: Vec<(Provider, String)>=Provider::ALL
        .iter()
        .filter_map(|p| env::var(p.env_var()).ok().map(|key| (*p, key)))
        .filter(|(_,key)| !key.is_empty())
        .collect();

    match found.as_slice() {
        [] =>{
            let names:Vec<_>=Provider::ALL.iter().map(|p| p.env_var()).collect();
            Err(anyhow::anyhow!("No API key found. Export one of: {}", names.join(", ")))
        }
        [(provider,key)] => Ok((*provider,key.clone())),
        many => {
            let names: Vec<_>=many.iter().map(|(p, _)| p.env_var()).collect();
            Err(anyhow::anyhow!(
                "Several API keys are set ({}). Unset all but one so the provider is unambiguous.",
                names.join(", ")
            ))
        }
    }
}



const PREAMBLE: &str="\
You are a research assistant responsible to search for literature search in Pubmed, 
PMC, and bioRxiv.org,  fill in the search queries, download the papers and analyze 
them if the users request. Ask the user one question at the time.

Add the query to the code, ask the limit of how many papers come back to the user. Also \
ask the year from which the search should start(e.g. 2020, only papers after 2020 are taken \
into consideration. After asking the user for the query/what wants to search, with the pubmed \
structure for queries.

Start by asking the user:
1. What is the topic of search
2. Which database or all to use (Pubmed, and bioRxiv.org).
3. The limit of papers, after year of publication.
4. Ask where is the output folder.
6. Ask whether the papers should be read and a report written, or only downloaded.
7. Ask whether the search results should be screened, that is read and filtered against \
    what he is looking for before anything is downloaded.

Rules:
- PubMed papers are saved as a pdf and a markdown file (1 pdf, 1 .md per paper).
- bioRxiv preprints are saved as a pdf and a plain text file extracted from that pdf \
    (1 pdf, 1 .txt per preprint). Do not promise the user markdown for bioRxiv.
- download_pubmed only takes PMC ids, download_biorxiv only takes DOIs. \
- Only one search (query) per database: Call search_pubmed once and search_biorxiv \
    once. Do not try a second query  if requested by the user, not because you think is better 
  If a search comes back empty, say so and ask the user how \
    to change the query instead of guessing at another one yourself. Think the query \
    through before you send it, because you get one.
- Screening is optional and it is asked for, never assumed: pass screen=true to \
    search_pubmed only when the user said yes. Screening reads every title and abstract \
    and reports only the papers that match, with a reason each, so with it a limit of 30 \
    or 50 papers is reasonable; without it ask for 10 or fewer, because every abstract \
    comes back whole.
- When screening was used, save the papers it dropped with save_report, as \
    out_of_scope_papers.md in the same output folder: one line per dropped paper with its DOI \
    and PMC id or PMID, its title and the reason it was dropped. The user has to be able \
    to check what the screening threw away.
- Reading the downloaded papers is optional and it is the slow, expensive step: \
    create_summary makes one model call per paper. Ask the user whether they want the \
    papers read and a report written, or only the files downloaded, and do not call \
    create_summary unless they said yes.
- When they do want it, call create_summary on the folder the papers are in before \
    writing anything about them, and build the report out of what it gives back. Say in \
    the report when a paper could not be read.
- When the user asks for a summary, a comparison or a report, write it with save_report \
    into the same folder as the papers, one call per paper or per section. Do not write \
    the summary in the reply.
- After saving: say which file was written, in which folder, \
    and report anything that failed, such as a paper that coulnt be downloaded.
- Always end your turn by writing something to the user, even when all the work went into \
    a file and even when there is little to say.
";

const SCREENER_PREAMBLE: &str="\
You are given what a reader is looking for, and the title and abstract of one paper. \
You decide whether that paper is worth their time.

Answer in one line and nothing else:
- KEEP: <up to fifteen words on what the paper does that matches>
- DROP: <up to fifteen words on why it does not match>

Rules:
- Judge the paper on what the abstract says it did, not on the words it happens to \
    contain. A paper that only uses a method in passing does not match a reader who \
    asked about that method itself.
- Judge only from the title and abstract you are given. Never use anything else you \
    know about the paper.
- When there is no abstract, or it is too vague to judge, answer KEEP: no abstract, \
    not screened. 
";

const SUMMARIZER_PREAMBLE: &str="\
You are given the text of one scientific paper and a description of what the reader \
wants out of it. You write a short summary of that paper and nothing else. You do not \
ask questions and you do not comment on the task.

Write it like this:
- First line: the DOI or the PMC id of the paper, and the year, as they appear in the text.
  Write unknown for whichever of them is not there.
- Then at most six sentences: what was done, on what material, with which tools, and what \
    was found.

Rules:
- Use only the text you are given. Never add anything you know about the paper, its \
    authors or its subject from anywhere else. You are summarising this text.
- Keep numbers, tool names, versions, accession numbers and species names exactly as \
    written in the text. 
- When something the reader asked for is not reported in the paper, say it.
- The text may have been cut off before the end, and text taken out of a pdf may be \
    garbled in places. Summarise what is there and do not guess at what is missing.
- If the text is too damaged or too short to summarise, say that instead of inventing \
    a summary.
";

const MAX_TURNS:usize=20;
const MAX_ATTEMPTS:u32 = 6;
const RATE_LIMIT_BACKOFF_BASE: u64 = 5;
const RATE_LIMIT_BACKOFF_CAP: u64 = 60;


// ----------------------------building the agents------------------------

//a subagent: a preamble and nothing else, no tools and no turn limit to speak of
fn worker(builder: AgentBuilder, preamble: &str) -> Agent {
    builder.preamble(preamble).build()
}

fn main_agent(builder: AgentBuilder, screener: Agent, summarizer: Agent) -> Agent {
    builder
        .preamble(PREAMBLE)
        .tool(SearchPubMed { screener: screener.clone() })
        .tool(DownloadPubMed)
        .tool(SearchBiorXvir { screener })
        .tool(DownloadBiorXvir)
        .tool(SaveReport)
        .tool(CreateSummary { summarizer })
        .default_max_turns(MAX_TURNS)
        .build()
}

fn build_agent(
    provider: Provider,
    key: String,
    http: ClientWithMiddleware,
) -> Result<Agent,anyhow::Error> {
    let agent=match provider {
        Provider::Anthropic => {
            let client = anthropic::Client::builder()
                .api_key(key)
                .anthropic_version(ANTHROPIC_VERSION_LATEST)
                .anthropic_beta("prompt-caching-2024-07-31")
                .http_client(http)
                .build()?;
            let worker_model = || AgentBuilder::new(client.completion_model(provider.worker_model()));

            //anthropic needs max_tokens on every request 
            let screener =worker(worker_model().max_tokens(16000), SCREENER_PREAMBLE);
            let summarizer=worker(worker_model().max_tokens(32000),SUMMARIZER_PREAMBLE);
            let main = AgentBuilder::new(
                client.completion_model(provider.model()).with_prompt_caching()
            ).max_tokens(64000);
            main_agent(main,screener,summarizer)
        }

        Provider::OpenAi=> {
            let client = openai::Client::builder().api_key(key).http_client(http).build()?;

            let screener = worker(client.agent(provider.worker_model()), SCREENER_PREAMBLE);
            let summarizer= worker(client.agent(provider.worker_model()), SUMMARIZER_PREAMBLE);

            main_agent(client.agent(provider.model()),screener,summarizer)
        }

        Provider::Moonshot=>{
            let client = moonshot::Client::builder().api_key(key).http_client(http).build()?;

            let screener = worker(client.agent(provider.worker_model()), SCREENER_PREAMBLE);
            let summarizer = worker(client.agent(provider.worker_model()), SUMMARIZER_PREAMBLE);

            main_agent(client.agent(provider.model()), screener, summarizer)
        }

        Provider::DeepSeek => {
            let client = deepseek::Client::builder().api_key(key).http_client(http).build()?;

            let screener =worker(client.agent(provider.worker_model()), SCREENER_PREAMBLE);
            let summarizer = worker(client.agent(provider.worker_model()), SUMMARIZER_PREAMBLE);

            main_agent(client.agent(provider.model()),screener, summarizer)
        }
    };
    Ok(agent)
}

#[tokio::main]
async fn main()->Result <(),anyhow::Error> {

    let retry_policy =ExponentialBackoff::builder()
        .retry_bounds(
            Duration::from_secs(RATE_LIMIT_BACKOFF_BASE),
            Duration::from_secs(RATE_LIMIT_BACKOFF_CAP),
        )
        .build_with_max_retries(MAX_ATTEMPTS);

    let http_client=ClientBuilder::new(reqwest::Client::new())
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();

    let (provider,key) = match detect_provider() {
        Ok(found) => found,
        Err(e)=>{
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let agent = build_agent(provider,key,http_client)?;

    println!();
    println!("Literature assistant: searches PubMed and bioRxiv, screens, downloads the papers");
    println!("and writes a summary into the folder you choose.");
    println!("Using {:?}: {} for the conversation, {} for reading the papers.",
        provider, provider.model(), provider.worker_model());
    println!("Say what you want to search for, or type exit to leave.");
    println!();

    ChatBotBuilder::new()
        .agent(agent)
        .max_turns(MAX_TURNS)
        .show_usage()
        .build()
        .run()
        .await?;
    Ok(())
}
