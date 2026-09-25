use pubmed_client::Client;
use pubmed_client::{PmcClient, PmcMarkdownConverter, HeadingStyle, ReferenceStyle};
use rig::integrations::cli_chatbot::ChatBotBuilder;
use rig::prelude::*;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::RetryTransientMiddleware;
use reqwest_retry::policies::ExponentialBackoff;
use std::env;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use rig::providers::openai;
use serde::Deserialize;
use serde_json::json;


//-----------------------------------PubMed--------------------------------------------------
#[derive(Deserialize)]
  pub struct SearchArgs {
    query: String,
    limit: usize,
    year: u32,
  }

  pub struct SearchPubMed;

  impl Tool for SearchPubMed {
    const NAME: &'static str="search_pubmed";
    type Error = pubmed_client::PubMedError;
    type Args = SearchArgs;
    type Output=String;   
    
    fn description(&self) -> String {
        "Search PubMed for papers that have free full text and were published after a \
        given year. Returns at most `limit` papers, one line each with the PMID, the PMC \
        id, the DOI and the title. The PMC id is what download_pubmed needs; a paper \
        without a PMC id cannot be downloaded.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": " A PubMed search query. Use PubMed syntax: field tags such as \
                    [Title/Abstract] or [MeSH Terms], and AND / OR / NOT to combine terms. Example: \
                    Cercospora beticola[Title/Abstract] AND effector[Title/Abstract]."
                },
                "year": {
                    "type": "integer",
                    "description" : "Year after which publications are searched for"
                },
                "limit": {
                    "type" : "integer",
                    "description" : "Maximum number of papers to return. Use 20 or fewer unless the user asks for more."
                },
            },
            "required": ["query", "limit","year"]
        })
    }

    async fn call(&self,_context: &mut ToolContext, args: Self::Args)-> Result<Self::Output, Self::Error>{
        
        progress(&format!("searching PubMed: {}", args.query));

        let mut results = String::new();

        let client = Client::new();
        let articles=client.pubmed
            .search()
            .query(args.query)
            .free_full_text_only()
            .published_after(args.year)
            .limit(args.limit)
            .search_and_fetch(&client.pubmed)
            .await?;

        for i in &articles {
            let pmc=i.pmc_id.as_deref().unwrap_or("none");
            let doi= i.doi.as_deref().unwrap_or("none");
            results.push_str(&format!("PMID {} | {} | doi {} | {}\n", i.pmid, pmc, doi, i.title));
        }
        Ok(results)
        } 
    }
    
#[derive(Deserialize)]
pub struct DownloadArgsPubMed {
    output_dir:String,
    papers_id:Vec<String>,
}

pub struct DownloadPubMed;

impl Tool for DownloadPubMed {
    const NAME: &'static str="download_pubmed";
    type Error = pubmed_client::PubMedError;
    type Args = DownloadArgsPubMed;
    type Output=String;
    
    fn description(&self) -> String {
      "Download papers from PubMed Central into a folder. Takes PMC ids from \
        search_pubmed results. For each paper it saves the PDF and a markdown version \
        of the full text. Returns one line per id saying whether it was saved and where, \
        or why it failed; papers that are not in PMC's open-access collection cannot \
        be downloaded.".to_string()
    }

     fn parameters(&self)->serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "output_dir": {
                "type" :"string",
                "description" : "Folder where the papers are saved. Use the folder the user gave; \
                                it is created if it does not exist."
                },
                "papers_id" : {
                "type" : "array",
                "items" :{"type" : "string"},
                "description" : "PMC ids to download, exactly as they appear in the search_pubmed results \
                (for example PMC7272562). At most 20 per call. Unless the user says otherwise" 
                },
            },
            "required": ["output_dir","papers_id"]
        })
    }

async fn call(&self,_context:&mut ToolContext,args:Self::Args)-> Result<Self::Output, Self::Error>{
    
    let client =PmcClient::new();
    let mut results_err=String::new();

    if let Err(e)=std::fs::create_dir_all(&args.output_dir) {
        return Ok(format!("could not create folder {}: {e}\n", args.output_dir));
    }

    for i in args.papers_id {
        if !i.starts_with("PMC") {
            results_err.push_str(&format!("{i}: not a PMC id\n"));
            continue;
        }
        progress(&format!("downloading {i} from PubMed Central"));
        if let Ok(full_text) = client.fetch_full_text(&i).await {
            if let Err(e) =client.download_files(&i, &args.output_dir).await {
                results_err.push_str(&format!("{i}: PDF failed: {e}\n"))
            }
            let converter =PmcMarkdownConverter::new()
                .with_include_metadata(true)
                .with_include_toc(true)
                .with_heading_style(HeadingStyle::ATX)
                .with_reference_style(ReferenceStyle::Numbered);

            let markdown= converter.convert(&full_text);
            let article_id = format!("{}.md",i);
            let article_success=format!("Downloaded markdown {}\n", i);
            if let Err(e) = std::fs::write(Path::new(&args.output_dir).join(&article_id), markdown) {
                results_err.push_str(&format!("{i}: markdown not saved {e}\n"));
            } else {
                results_err.push_str(&article_success);
            }

        } else {
            results_err.push_str(&format!("{i}: not available open-acess text\n"));
            } 
        };
        Ok(results_err)
    }
}

//-----------------------------------BioarXiv------------------------------------------------------------
pub struct SearchBiorXvir;

impl Tool for SearchBiorXvir {
    const NAME: &'static str="search_biorxiv";
    type Error =pubmed_client::PubMedError;
    type Args = SearchArgs;
    type Output=String;

    fn description(&self) -> String {
      "Search bioRxiv preprints published in or after a given year, through Europe PMC. \
        Returns at most `limit` preprints, one line each with the DOI, the year and the \
        title. The DOI is what download_biorxiv needs. Preprints are not peer reviewed, \
        and the same work may also appear in PubMed as a published paper.".to_string()
    }

     fn parameters(&self)->serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The topic to search for, in Europe PMC syntax: plain words, \
                    AND / OR / NOT to combine them, and field prefixes such as TITLE: or ABSTRACT:. \
                    Do not use PubMed tags like [Title/Abstract], and do not add a bioRxiv or a year \
                    filter here; the tool adds those itself. Example: Cercospora beticola AND effector."
                },
                "year":{
                    "type": "integer",
                    "description" : "Year from which preprints are searched for"
                },
                "limit": {
                    "type":"integer",
                    "description" : "Maximum number of preprints to return. Use 20 or fewer unless the user asks for more."
                },
            },
            "required": ["query","limit","year"]
        })
    }

     async fn call(&self,_context: &mut ToolContext, args: Self::Args)-> Result<Self::Output, Self::Error>{
        
        progress(&format!("searching bioRxiv: {}", args.query));

        let mut results_bio=String::new();

         let query= format!(
            "(SRC:PPR AND PUBLISHER:\"bioRxiv\") AND ({}) AND PUB_YEAR:[{} TO *]",
            args.query, args.year
        );

        let client_bio = Client::new();
        let articles_bio = client_bio.europe_pmc
            .search(&query, args.limit)
            .await?;

        for i in &articles_bio {
            let doi =i.doi.as_deref().unwrap_or("none");
            let year = i.pub_year.as_deref().unwrap_or("none");
            let title = i.title.as_deref().unwrap_or("none");
            results_bio.push_str(&format!("doi {} | {} | {}\n", doi, year, title));
        }
        Ok(results_bio)
    } 
}

pub struct DownloadBiorXvir;

impl Tool for DownloadBiorXvir {
    const NAME: &'static str="download_biorxiv";
    type Error =std::io::Error;
    type Args = DownloadArgsPubMed;
    type Output=String;

    fn description(&self) -> String {
      "Download bioRxiv preprints into a folder. Takes DOIs from search_biorxiv \
        results. For each paper it downloads the latest version's PDF from biorxiv.org \
        and saves it together with a text version extracted from the PDF. Returns one \
        line per DOI saying whether it was saved and where, or why it failed.".to_string()
    }

     fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "output_dir":{
                "type" : "string",
                "description" : "Folder where the papers are saved. Use the folder the user gave; \
                                it is created if it does not exist."
                },
                "papers_id" : {
                "type" :"array",
                "items" : {"type" : "string"},
                "description" : "bioRxiv DOIs to download, exactly as they appear in the search_biorxiv \
                results (for example 10.1101/2024.01.15.575678). At most 20 per call unless the user \
                says otherwise."
                },
            },
            "required": ["output_dir","papers_id"]
        })
    }

//search bioarxivwith request limitation for the 429 error(too many request). The interval may be changed
async fn call(&self,_context: &mut ToolContext, args: Self::Args)-> Result<Self::Output, Self::Error>{
    
    let client = match reqwest::Client::builder()
        .user_agent(BIORXIV_USER_AGENT)
        .build() {
            Ok(client) => client,
            Err(e) => return Ok(format!("could not start the downloader: {e}\n")),
        };

    let mut results_err = String::new();

    if let Err(e) = std::fs::create_dir_all(&args.output_dir) {
        return Ok(format!("could not create folder {}: {e}\n", args.output_dir));
    }

    'papers: for i in args.papers_id {
        if !i.starts_with("10.") {
            results_err.push_str(&format!("{i}: not a DOI id\n"));
            continue;
        }

        let url=format!("https://www.biorxiv.org/content/{i}.full.pdf");
        let paper_name= i.replace('/', "_");

        progress(&format!("downloading {i} from bioRxiv"));
        tokio::time::sleep(Duration::from_secs(BIORXIV_PAUSE)).await;

        let mut attempt = 0;
        let response = loop {
            attempt+=1;
            match client.get(&url).send().await {
                Ok(response) => {
                    if response.status().as_u16() ==429 && attempt<BIORXIV_ATTEMPTS {
                        let wait = retry_after(&response)
                            .unwrap_or(BIORXIV_RETRY_BASE*attempt as u64);
                        progress(&format!("bioRxiv is rate limiting, waiting {wait}s"));
                        tokio::time::sleep(Duration::from_secs(wait)).await;
                        continue;
                    }
                    break response;
                }
                Err(e) => {
                    results_err.push_str(&format!("{i}: request failed: {e}\n"));
                    continue 'papers;
                }
            }
        };

        if response.status().as_u16() == 429 {
            results_err.push_str(&format!("{i}: bioRxiv is rate limiting, still refused after \
                {BIORXIV_ATTEMPTS} tries. Do not retry this tool now, tell the user to try \
                again in a few minutes.\n"));
            continue;
        }

        if !response.status().is_success() {
            results_err.push_str(&format!("{i}: not downloaded, server answered {}\n", response.status()));
            continue;
        }

        let pdf_bytes = match response.bytes().await {
            Ok(pdf_bytes) => pdf_bytes,
            Err(e) => {
                results_err.push_str(&format!("{i}: download interrupted: {e}\n"));
                continue;
            }
        };

        let pdf_path = Path::new(&args.output_dir).join(format!("{paper_name}.pdf"));
        if let Err(e)=std::fs::write(&pdf_path, &pdf_bytes) {
            results_err.push_str(&format!("{i}: pdf not saved {e}\n"));
            continue;
        }

        let text=match pdf_extract::extract_text_from_mem(&pdf_bytes) {
            Ok(text)=>text,
            Err(e)=>{
                results_err.push_str(&format!("{i}: pdf saved, but no text could be read from it: {e}\n"));
                continue;
            }
        };

        let text_path=Path::new(&args.output_dir).join(format!("{paper_name}.txt"));
        if let Err(e)=std::fs::write(&text_path, text) {
            results_err.push_str(&format!("{i}: text not saved {e}\n"));
        } else {
            results_err.push_str(&format!("Downloaded {paper_name}.pdf and {paper_name}.txt\n"));
        }
        }
        Ok(results_err)
    }
}

//-----------------------------------Report--------------------------------------------------
#[derive(Deserialize)]
pub struct SaveReportArgs {
    output_dir: String,
    file_name: String,
    content: String,
    mode: String,
}

pub struct SaveReport;

impl Tool for SaveReport {
    const NAME: &'static str="save_report";
    type Error=std::io::Error;
    type Args=SaveReportArgs;
    type Output=String;

    fn description(&self) -> String {
      "Save a summary, report or notes as a text file in the output folder. Use this \
        instead of writing a long summary in the reply: the reply should only say what \
        was saved and where. Write the report in pieces, one call per paper or per \
        section, so that nothing gets cut off. The first call uses mode \"new\", every \
        later call about the same file uses mode \"append\".".to_string()
    }

    fn parameters(&self) ->serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "output_dir": {
                "type" : "string",
                "description" : "Folder where the report is saved. Use the same folder the papers \
                were downloaded to; it is created if it does not exist."
                },
                "file_name" : {
                "type" :"string",
                "description":"Name of the file, with its extension, for example summary.md. \
                Keep using the same name for every part of the same report."
                },
                "content" : {
                "type" : "string",
                "description" : "The text to write, in markdown. One paper or one section per call."
                },
                "mode":{
                "type" : "string",
                 "enum" : ["new","append"],
                "description" : "\"new\" writes the file from scratch and erases anything already \
                in it, so use it only for the first piece of a report. Every later call about the \
                same file must use \"append\", which adds to the end."
                },
            },
            "required": ["output_dir","file_name","content","mode"]
        })
    }

async fn call(&self,_context: &mut ToolContext, args: Self::Args)-> Result<Self::Output, Self::Error>{

    if let Err(e) = std::fs::create_dir_all(&args.output_dir) {
        return Ok(format!("could not create folder {}: {e}\n", args.output_dir));
    }

    progress(&format!("writing {}", args.file_name));

    let report_path = Path::new(&args.output_dir).join(&args.file_name);
    let mut text = args.content;
    if !text.ends_with('\n') {
        text.push('\n');
    }

    if args.mode=="new" {
        if let Err(e)=std::fs::write(&report_path, &text) {
            return Ok(format!("report not saved: {e}\n"));
        }
        Ok(format!("Report started in {}\n", report_path.display()))
    } else {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&report_path);

        match file {
            Ok(mut file) => {
                if let Err(e) = file.write_all(text.as_bytes()) {
                    return Ok(format!("report not saved: {e}\n"));
                }
                Ok(format!("Added to the report in {}\n", report_path.display()))
            }
            Err(e)=>Ok(format!("could not open {}: {e}\n", report_path.display())),
        }
    }
    }
}

const PREAMBLE: &str="\
You are a research assistant responsible to search for literature search in Pubmed, 
PMC, and bioRxiv.org,  fill in the search queries, download the papers and analyze 
them if the users request. Ask the user one question at the time, then perform the search
and download the papers, etc. 

Add the query to the code, ask the limit of how many papers come back to the user. Also \
    ask the year from which the search should start(e.g. 2020, only papers after 2020 are taken \
    into consideration. After asking the user for the query/what he wants to search, with the pubmed \
    structure for queries


Start by asking the user:
1. What he wants to search about.
2. Which database or all to use (Pubmed, and bioRxiv.org).
3. if is pubmed ask the limit of papers, after year of publication.
4. Ask where is the output folder.
5. Ask about speficially things he may want from the papers.

Rules:
- PubMed papers are saved as a pdf and a markdown file (1 pdf, 1 .md per paper).
- bioRxiv preprints are saved as a pdf and a plain text file extracted from that pdf \
    (1 pdf, 1 .txt per preprint), because a preprint pdf has no structured text to turn \
    into markdown. Do not promise the user markdown for bioRxiv.
- download_pubmed only takes PMC ids, download_biorxiv only takes DOIs. Never send an id \
    to the wrong tool.
- When the user asks for a summary, a comparison or a report, write it with save_report \
    into the same folder as the papers, one call per paper or per section. Do not write \
    the summary in the reply.
- After saving, the reply is only a receipt: say which file was written, in which folder, \
    and report anything that failed, such as a paper that could not be downloaded.
- Always end your turn by writing something to the user, even when all the work went into \
    a file and even when there is little to say. Never finish a turn in silence: the user \
    only sees what you write, not the tools you called.
";

const MAX_TURNS: usize = 20;
const MAX_ATTEMPTS: u32 = 6;
const RATE_LIMIT_BACKOFF_BASE: u64 = 5;
const RATE_LIMIT_BACKOFF_CAP: u64 = 60;
const BIORXIV_USER_AGENT: &str = "lit_agent/0.1 (literature search agent)";
const BIORXIV_PAUSE: u64 = 1;
const BIORXIV_ATTEMPTS: u32 = 3;
const BIORXIV_RETRY_BASE: u64 = 10;


fn retry_after(response: &reqwest::Response)->Option<u64> {
    response
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

fn progress(what: &str) {
    println!("  {what}");
}

#[tokio::main]
async fn main() -> Result <(), anyhow::Error> {

    let retry_policy = ExponentialBackoff::builder()
        .retry_bounds(
            Duration::from_secs(RATE_LIMIT_BACKOFF_BASE),
            Duration::from_secs(RATE_LIMIT_BACKOFF_CAP),
        )
        .build_with_max_retries(MAX_ATTEMPTS);

    let http_client=ClientBuilder::new(reqwest::Client::new())
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();

    let api_key= match env::var("OPENAI_API_KEY") {
        Ok(api_key)=>api_key,
        Err(_)=>{
            eprintln!("OPENAI_API_KEY is not set. Export it first:");
            eprintln!("  export OPENAI_API_KEY='sk-...'");
            std::process::exit(1);
        }
    };

    let agent=openai::Client::builder()
            .api_key(api_key)
            .http_client(http_client)
            .build()?
            .agent(openai::completion::GPT_5_6)
            .preamble(PREAMBLE)
            .tool(SearchPubMed)
            .tool(DownloadPubMed)
            .tool(SearchBiorXvir)
            .tool(DownloadBiorXvir)
            .tool(SaveReport)
            .default_max_turns(MAX_TURNS)
            .build();

    println!();
    println!("Literature assistant: searches PubMed and bioRxiv, downloads the papers");
    println!("and writes a summary into the folder you choose.");
    println!("Say what you want to search for, or type exit to leave.");
    println!();

    ChatBotBuilder::new()
        .agent(agent)
        .max_turns(MAX_TURNS)
        //.show_usage()
        .build()
        .run()
        .await?;

    Ok(())

}
