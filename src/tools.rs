use pubmed_client::Client;
use pubmed_client::europe_pmc::{EuropePmcSearchOptions, ResultType};
use pubmed_client::{PmcClient, PmcMarkdownConverter, HeadingStyle, ReferenceStyle};
use rig::agent::Agent;
use rig::prelude::*;
use serde::Deserialize;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BIORXIV_USER_AGENT: &str = "lit_agent/0.1 (literature search agent)";
const BIORXIV_PAUSE: u64 = 10;
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

//-----------------------------------PubMed--------------------------------------------------
#[derive(Deserialize)]
  pub struct SearchPubMedArgs {
    query: String,
    limit: usize,
    year: u32,
    screen: bool,
  }

  //this one carries a subagent: with screen=true it reads the titles and abstracts 
  pub struct SearchPubMed {
    pub screener: Agent,
  }

  impl Tool for SearchPubMed {
    const NAME: &'static str="search_pubmed";
    type Error = pubmed_client::PubMedError;
    type Args = SearchPubMedArgs;
    type Output=String;   
    
    fn description(&self) -> String {
        "Search PubMed for papers that have free full text and were published after a \
        given year. With screen=false it returns every paper it found, each as a line \
        with the PMID, the PMC id, the DOI and the title, followed by its abstract; judge \
        those papers by their abstracts and not by their titles. With screen=true it \
        reads the titles and abstracts itself and reports only the papers that match the \
        query, one line each with a short reason, and says how many it dropped. The PMC \
        id is what download_pubmed needs; a paper without a PMC id cannot be \
        downloaded.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": " A PubMed search query. Use PubMed syntax: field tags such as \
                    [Title/Abstract] or [MeSH Terms], and AND / OR / NOT to combine terms. Example: \
                    Cercospora beticola[Title/Abstract] AND (effector[Title/Abstract] OR eff*[Title/Abstract])."
                },
                "year": {
                    "type": "integer",
                    "description" : "Year after which publications are searched for"
                },
                "limit": {
                    "type" : "integer",
                    "description" : "Maximum number of papers to look at. Without screening each one \
                    comes back with its abstract, so ask for 10 or fewer; with screening the \
                    abstracts are read inside the tool and never come back, so 30 or 50 is fine."
                },
                "screen": {
                    "type" : "boolean",
                    "description" : "true to have the papers read and filtered against the query \
                    before they are reported, false to get everything the search found. Only set \
                    it to true when the user asked for screening: it costs one extra model call \
                    per paper."
                },
            },
            "required": ["query", "limit","year","screen"]
        })
    }

    async fn call(&self,_context: &mut ToolContext, args: Self::Args)-> Result<Self::Output, Self::Error>{
        
        progress(&format!("searching PubMed: {}", args.query));
        let mut results = String::new();
        let client = Client::new();
        let articles=client.pubmed
            .search()
            .query(&args.query)
            .free_full_text_only()
            .published_after(args.year)
            .limit(args.limit)
            .search_and_fetch(&client.pubmed)
            .await?;

        let mut dropped = 0;
        let mut dropped_list = String::new();
        for i in &articles {
            let pmc=i.pmc_id.as_deref().unwrap_or("none");
            let doi= i.doi.as_deref().unwrap_or("none");
            let summary = i.abstract_text.as_deref().unwrap_or("no abstract available");
            let heading = format!("PMID {} | {} | doi {} | {}", i.pmid, pmc, doi, i.title);

            if !args.screen {
                results.push_str(&format!("{heading}\n{summary}\n\n"));
                continue;
            }

            //screening: the abstract goes to the subagent and stays there. only the verdict comes back
            let verdict = self.screener
                .prompt(format!("The reader is looking for: {}\n\nTitle: {}\n\nAbstract: {}",
                    args.query, i.title, summary)).await;

            match verdict {
                Ok(verdict) if verdict.trim_start().to_uppercase().starts_with("DROP") => {
                    dropped += 1;
                    dropped_list.push_str(&format!("{heading}\n{}\n\n", verdict.trim()));
                }
                Ok(verdict) => results.push_str(&format!("{heading}\n{}\n\n", verdict.trim())),
                Err(e) => results.push_str(&format!("{heading}\nnot screened ({e}), judge it yourself:\n{summary}\n\n")),
            }
        }

        if args.screen {
            results.push_str(&format!(
                "{} papers found, {} kept, {} dropped as not matching the query\n",
                articles.len(), articles.len()-dropped, dropped
            ));
            if dropped > 0 {
                results.push_str(&format!("\ndropped papers:\n\n{dropped_list}"));
            }
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
        search_pubmed results. For each paper the folder gets the PDF and a markdown \
        version of the full text; everything else PMC supplies with the paper, such as \
        figures, supplementary tables and its xml, goes into an other_files subfolder. \
        Returns one line per id saying whether it was saved and where, or why it failed; \
        papers that are not in PMC's open-access collection cannot be downloaded.".to_string()
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

async fn call(&self,_context: &mut ToolContext,args: Self::Args)-> Result<Self::Output, Self::Error>{
    
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
        if let Ok(full_text)=client.fetch_full_text(&i).await {
            let other_files = Path::new(&args.output_dir).join("other_files");
            //what became of the pdf, so the line at the end of the loop can say it. not
            //every paper has one: a deposited manuscript often has only figures and text
            let mut pdf = String::from("no pdf, PMC has none for this id");
            match client.download_files(&i, &other_files).await {
                Err(e) => pdf = format!("no pdf: {e}"),
                Ok(saved) => {
                    for file in &saved {
                        let from = Path::new(file);
                        let name = from.file_name().unwrap_or_default().to_string_lossy().to_string();
                        let is_the_paper = name.starts_with(&i) && name.ends_with(".pdf");
                        if is_the_paper {
                            let to = Path::new(&args.output_dir).join(&name);
                            pdf = match std::fs::rename(from, &to) {
                                Ok(()) => name,
                                Err(e) => format!("pdf left in other_files: {e}"),
                            };
                        }
                    }
                }
            }
            let converter=PmcMarkdownConverter::new()
                .with_include_metadata(true)
                .with_include_toc(true)
                .with_heading_style(HeadingStyle::ATX)
                .with_reference_style(ReferenceStyle::Numbered);

            let markdown= converter.convert(&full_text);
            let article_id = format!("{}.md",i);
            //one line per paper saying what actually landed
            let article_success=format!("{i}: saved {article_id} and {pdf}\n");
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
#[derive(Deserialize)]
pub struct SearchBiorxivArgs {
    query: String,
    limit: usize,
    year: u32,
    screen: bool,
}

pub struct SearchBiorXvir {
    pub screener: Agent,
}

impl Tool for SearchBiorXvir {
    const NAME: &'static str="search_biorxiv";
    type Error =pubmed_client::PubMedError;
    type Args = SearchBiorxivArgs;
    type Output=String;

    fn description(&self) -> String {
      "Search bioRxiv preprints published in or after a given year, through Europe PMC. \
        With screen=false it returns every preprint it found, each as a line with the DOI, \
        the year and the title, followed by its abstract. With screen=true it reads those \
        abstracts itself and reports only the preprints that match the query, one line \
        each with a short reason, and says how many it dropped. The DOI is what \
        download_biorxiv needs. Preprints are not peer reviewed, and the same work may \
        also appear in PubMed as a published paper.".to_string()
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
                    "type" : "integer",
                    "description" : "Maximum number of preprints to look at. Without screening each \
                    one comes back with its abstract, so ask for 10 or fewer; with screening the \
                    abstracts are read inside the tool and never come back, so 30 or 50 is fine."
                },
                "screen": {
                    "type" : "boolean",
                    "description" : "true to have the preprints read and filtered against the query \
                    before they are reported, false to get everything the search found. Only set it \
                    to true when the user asked for screening: it costs one extra model call per \
                    preprint."
                },
            },
            "required": ["query","limit","year","screen"]
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
            .search_all(&query, args.limit, &EuropePmcSearchOptions {
                result_type: ResultType::Core,
                ..Default::default()
            })
            .await?;

        let mut dropped = 0;
        let mut dropped_list = String::new();

        for i in &articles_bio {
            let doi =i.doi.as_deref().unwrap_or("none");
            let year = i.pub_year.as_deref().unwrap_or("none");
            let title = i.title.as_deref().unwrap_or("none");
            let summary = i.extra
                .get("abstractText")
                .and_then(|a| a.as_str())
                .unwrap_or("no abstract available");
            let heading = format!("doi {} | {} | {}", doi, year, title);

            if !args.screen {
                results_bio.push_str(&format!("{heading}\n{summary}\n\n"));
                continue;
            }

            //the abstract goes to the subagent and stays there, only the verdict comes back
            match self.screener.prompt(format!(
                    "The reader is looking for: {}\n\nTitle: {}\n\nAbstract: {}",
                    args.query, title, summary)).await {
                Ok(verdict) if verdict.trim_start().to_uppercase().starts_with("DROP") => {
                    dropped += 1;
                    dropped_list.push_str(&format!("{heading}\n{}\n\n", verdict.trim()));
                }
                Ok(verdict) => results_bio.push_str(&format!("{heading}\n{}\n\n", verdict.trim())),
                Err(e) => results_bio.push_str(&format!("{heading}\nnot screened ({e}), judge it yourself:\n{summary}\n\n")),
            }
        }

        if args.screen {
            results_bio.push_str(&format!(
                "{} preprints found, {} kept, {} dropped as not matching the query\n",
                articles_bio.len(), articles_bio.len()-dropped, dropped
            ));
            if dropped > 0 {
                results_bio.push_str(&format!("\ndropped preprints:\n\n{dropped_list}"));
            }
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
                "output_dir": {
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

    progress(&format!("{} {}", if args.mode=="new" {"writing"} else {"adding to"}, args.file_name));

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

//----------------------------------------summary---------------------------------------------
#[derive(Deserialize)]
pub struct SummaryArgs {
    output_dir: String,
    focus: String,
}

pub struct CreateSummary{
    pub summarizer: Agent,
}

impl Tool for CreateSummary {
    const NAME: &'static str="create_summary";
    type Error=std::io::Error;
    type Args=SummaryArgs;
    type Output=String;

 fn description(&self) -> String {
      "Read every paper already downloaded into a folder and return a short summary of \
        each one. Each summary is read out of the file itself, not out of the title, and \
        starts with the paper's DOI or PMC id and its year. Call this after downloading \
        and before writing any report, and build the report out of what comes back. Only \
        the .md and .txt files are read; a paper whose text could not be read is reported \
        as such instead of being summarised.".to_string()
    }
    fn parameters(&self) ->serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "output_dir": {
                "type" : "string",
                "description" : "Folder where the Summary for all papers is saved. It is the \
                same folder wehere the  papers were downloaded to."
                },
                "focus" : {
                "type" : "string",
                "description" : "The text to write, in markdown. One paper or one section per call."
                },
            },
            "required": ["output_dir","focus"]
        })
    }

async fn call(&self,_context: &mut ToolContext, args: Self::Args)-> Result<Self::Output, Self::Error>{


    let folder_path = match std::fs::read_dir(&args.output_dir) {
        Ok(folder_path)=>folder_path,
        Err(e) =>return Ok(format!("{}: could not read file {e}\n", args.output_dir))
    };

    let mut unique_papers: Vec<PathBuf> = Vec::new();
    for i in folder_path {
        let Ok(i) = i else { continue; };
        let name = i.file_name().to_string_lossy().to_string();
        let keep = (name.starts_with("PMC") && name.ends_with(".md")) || (name.starts_with("10.") && name.ends_with(".txt"));

        if keep {
            unique_papers.push(i.path());     
        }
    }

    let mut summaries = String::new();
    for i in &unique_papers {
        progress(&format!("summarising {}", i.file_name().unwrap_or_default().to_string_lossy()));
        let text = match std::fs::read_to_string(i) {
            Ok(text) => text,
            Err(e) => {
                summaries.push_str(&format!("{}: Could not be read: {e}\n", i.display())); 
                continue;
            }            
        };

        //truncate to maximum size before send it to the model, build the prompt, and ask the agent
        let text:String =text.chars().take(150000).collect();
        let prompt_subagent=format!("The reader wants: {} \n\nPaper text:\n{}", args.focus, text);
        let name = i.file_name().unwrap_or_default().to_string_lossy();

        match self.summarizer.prompt(prompt_subagent).await {
            Ok(summary) => summaries.push_str(&format!("## {name}\n{summary}\n\n")),
            Err(e) => summaries.push_str(&format!("## {name}\ncould not be summarised: {e}\n\n")),
        }
      }
    Ok(summaries)
    }
}
