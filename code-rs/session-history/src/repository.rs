use crate::{
    Action, Error, Request, Result, Settings, Store, files, import, normalize, required, sqlite,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Session {
    summary: Value,
    rollouts: Vec<PathBuf>,
}
struct Context {
    store: Store,
    sessions: BTreeMap<String, Session>,
    sources: Vec<String>,
    warnings: Vec<String>,
    history: Option<PathBuf>,
    manifest: Option<Value>,
}
pub(crate) struct Repository {
    contexts: Vec<Context>,
    settings: Settings,
    warnings: Vec<String>,
}

impl Repository {
    pub(crate) async fn discover(settings: Settings) -> Result<Self> {
        let mut stores = settings.stores.clone();
        let mut warnings = Vec::new();
        if let Ok(entries) = std::fs::read_dir(settings.data_dir.join("datasets")) {
            let mut roots: Vec<_> = entries.flatten().collect();
            roots.sort_by_key(|e| e.file_name());
            for entry in roots {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(suffix) = name.strip_prefix("openai-") else {
                    continue;
                };
                if import::dataset_id(suffix).is_err()
                    || import::read_manifest(&entry.path()).is_err()
                {
                    warnings.push(format!("export dataset unavailable: {name}"));
                    continue;
                }
                if stores.iter().any(|store| store.id == name) {
                    warnings.push(format!("duplicate store ID: {name}"));
                    continue;
                }
                stores.push(Store {
                    id: name,
                    root: entry.path().canonicalize()?,
                });
            }
        }
        let mut contexts = Vec::new();
        for store in stores {
            contexts.push(discover_store(store).await?);
        }
        Ok(Self {
            contexts,
            settings,
            warnings,
        })
    }

    fn context(&self, id: &str) -> Result<&Context> {
        self.contexts
            .iter()
            .find(|ctx| ctx.store.id == id)
            .ok_or_else(|| Error::new("store_not_found", format!("unknown store: {id}")))
    }

    fn resolve(&self, reference: &str, store_id: Option<&str>) -> Result<(&Context, &Session)> {
        let (store, id) = reference
            .split_once(':')
            .map(|(s, id)| (Some(s), id))
            .unwrap_or((store_id, reference));
        if let Some(store) = store {
            if store_id.is_some_and(|specified| specified != store) {
                return Err(Error::new(
                    "invalid_argument",
                    "store_id conflicts with qualified session_ref",
                ));
            }
            let context = self.context(store)?;
            let session = context.sessions.get(id).ok_or_else(|| {
                Error::new("session_not_found", format!("unknown session: {reference}"))
            })?;
            return Ok((context, session));
        }
        let mut found = self
            .contexts
            .iter()
            .filter_map(|ctx| ctx.sessions.get(id).map(|session| (ctx, session)));
        let first = found.next().ok_or_else(|| {
            Error::new("session_not_found", format!("unknown session: {reference}"))
        })?;
        if found.next().is_some() {
            return Err(Error::new(
                "ambiguous_session",
                "session ID occurs in more than one store; use a store-qualified ref from list_sessions",
            ));
        }
        Ok(first)
    }

    pub(crate) async fn execute(&self, args: &Request) -> Result<Value> {
        let result = match args.action {
            Action::ListStores => {
                json!({"stores":self.contexts.iter().map(|ctx|json!({"store_id":ctx.store.id,"root":ctx.store.root,"sources":ctx.sources,"warnings":ctx.warnings})).collect::<Vec<_>>(),"count":self.contexts.len(),"warnings":self.warnings,"import_inbox":self.settings.data_dir.join("inbox")})
            }
            Action::ListSessions => self.list_sessions(args)?,
            Action::GetSession | Action::GetTurn => self.get_session(args).await?,
            Action::SearchMessages => self.search(args).await?,
            Action::InspectStore => {
                self.inspect(required(&args.store_id, "store_id")?, args.deep)
                    .await?
            }
            Action::ImportOpenaiExport => {
                return Err(Error::new(
                    "internal_error",
                    "import must use the import entry point",
                ));
            }
        };
        // Records are redacted before pagination. Re-redacting a shortened marker
        // here would expand it again and invalidate the requested content budget.
        Ok(result)
    }

    fn selected(&self, args: &Request) -> Result<Vec<(&Context, &Session)>> {
        if let Some(reference) = &args.session_ref {
            return Ok(vec![self.resolve(reference, args.store_id.as_deref())?]);
        }
        let contexts = if let Some(id) = &args.store_id {
            vec![self.context(id)?]
        } else {
            self.contexts.iter().collect()
        };
        let mut sessions: Vec<_> = contexts
            .into_iter()
            .flat_map(|ctx| ctx.sessions.values().map(move |session| (ctx, session)))
            .filter(|(_, session)| {
                let s = &session.summary;
                args.cwd_contains.as_ref().is_none_or(|needle| {
                    fold(s["cwd"].as_str().unwrap_or("")).contains(&fold(needle))
                }) && args
                    .since_ms
                    .is_none_or(|since| s["created_at_ms"].as_i64().unwrap_or(0) >= since)
                    && args
                        .until_ms
                        .is_none_or(|until| s["created_at_ms"].as_i64().unwrap_or(0) <= until)
                    && args
                        .archived
                        .is_none_or(|archived| s["archived"].as_bool() == Some(archived))
                    && args
                        .source_kind
                        .as_ref()
                        .is_none_or(|kind| s["source_kind"].as_str().unwrap_or("").contains(kind))
            })
            .collect();
        sessions.sort_by(|(_, a), (_, b)| {
            updated(&b.summary)
                .cmp(&updated(&a.summary))
                .then_with(|| b.summary["ref"].as_str().cmp(&a.summary["ref"].as_str()))
        });
        Ok(sessions)
    }

    fn list_sessions(&self, args: &Request) -> Result<Value> {
        let sessions = self.selected(args)?;
        let total = sessions.len();
        let items: Vec<_> = sessions
            .into_iter()
            .skip(args.offset)
            .take(args.limit.unwrap_or(50))
            .map(|(_, s)| s.summary.clone())
            .collect();
        let next = args.offset.saturating_add(items.len());
        Ok(
            json!({"count":items.len(),"total":total,"next_offset":if next<total{Some(next)}else{None},"items":items}),
        )
    }

    async fn get_session(&self, args: &Request) -> Result<Value> {
        let (ctx, session) = self.resolve(
            required(&args.session_ref, "session_ref")?,
            args.store_id.as_deref(),
        )?;
        let id = session.summary["session_id"].as_str().unwrap_or_default();
        let turn = if args.action == Action::GetTurn {
            Some(required(&args.turn_id, "turn_id")?)
        } else {
            None
        };
        let mut page = normalize::Page::new(
            if turn.is_some() { 0 } else { args.offset },
            args.limit.unwrap_or(if turn.is_some() { 200 } else { 50 }),
            args.max_chars.unwrap_or(50_000),
        );
        let source = visit_items(ctx, session, args.include_tools, &mut |item| {
            if turn.is_some_and(|turn| item["turn_id"].as_str() != Some(turn)) {
                return true;
            }
            page.push(item)
        })
        .await?;
        let turn_info =
            if let Some(turn) = turn {
                let info = sqlite::turn(&ctx.store, id, turn).await?;
                if info.is_none() && page.matched == 0 {
                    return Err(Error::new(
                        if source == "history" {
                            "turn_unsupported"
                        } else {
                            "turn_not_found"
                        },
                        "no retrievable items for this turn",
                    ));
                }
                Some(info.unwrap_or_else(
                    || json!({"store_id":ctx.store.id,"session_id":id,"turn_id":turn}),
                ))
            } else {
                None
            };
        let mut result = page.output();
        result["session"] = session.summary.clone();
        result["content_source"] = json!(source);
        if let Some(turn) = turn_info {
            result["turn"] = turn;
        }
        Ok(result)
    }

    async fn search(&self, args: &Request) -> Result<Value> {
        let query = fold(required(&args.query, "query")?);
        let limit = args.limit.unwrap_or(20);
        let mut matches = Vec::new();
        for (ctx, session) in self.selected(args)? {
            visit_items(ctx,session,args.include_tools,&mut |item| {
                if args.role.as_ref().is_some_and(|role|item["role"].as_str()!=Some(role)) {return true;}
                let text=normalize::searchable(&item);
                let Some(excerpt)=excerpt(&text,&query) else {return true;};
                matches.push(normalize::clean(json!({"session":session.summary,"kind":item["kind"],"role":item["role"],"turn_id":item["turn_id"],"ordinal":item["ordinal"],"tool_name":item["tool_name"],"excerpt":excerpt,"is_active_branch":item["is_active_branch"],"parent_item_id":item["parent_item_id"]})));
                matches.len()<limit
            }).await?;
            if matches.len() >= limit {
                return Ok(json!({"count":matches.len(),"matches":matches,"truncated":true}));
            }
        }
        Ok(json!({"count":matches.len(),"matches":matches,"truncated":false}))
    }

    async fn inspect(&self, id: &str, deep: bool) -> Result<Value> {
        let ctx = self.context(id)?;
        let paths: Vec<_> = ctx
            .sessions
            .values()
            .flat_map(|s| s.rollouts.iter())
            .collect();
        let mut result = json!({"store_id":id,"root":ctx.store.root,"sources":ctx.sources,"warnings":ctx.warnings,"session_count":ctx.sessions.len(),"rollout_count":paths.len(),"sqlite":sqlite::inspect(&ctx.store).await?});
        if let Some(history) = &ctx.history {
            result["history"] = files::inspect(history, true)?;
        }
        if let Some(manifest) = &ctx.manifest {
            result["openai_export"] = manifest.clone();
        }
        if deep {
            let mut reports = Vec::new();
            for path in paths.iter().take(500) {
                reports.push(files::inspect(path, false)?);
            }
            result["rollouts"] = json!(reports);
            result["rollouts_truncated"] = json!(paths.len() > 500);
        }
        Ok(result)
    }
}

fn updated(summary: &Value) -> i64 {
    summary["updated_at_ms"]
        .as_i64()
        .or(summary["created_at_ms"].as_i64())
        .unwrap_or(0)
}

async fn discover_store(store: Store) -> Result<Context> {
    let mut ctx = Context {
        store,
        sessions: BTreeMap::new(),
        sources: Vec::new(),
        warnings: Vec::new(),
        history: None,
        manifest: None,
    };
    for name in ["state_5.sqlite", "thread_history_1.sqlite", "history.jsonl"] {
        if ctx.store.root.join(name).is_file() {
            ctx.sources.push(name.into());
        }
    }
    if ctx.store.root.join("openai-export.json").is_file() {
        match import::read_manifest(&ctx.store.root) {
            Ok(value) => {
                ctx.manifest = Some(value);
                ctx.sources.push("openai_export".into());
            }
            Err(error) => ctx.warnings.push(error.to_string()),
        }
    }
    match sqlite::sessions(&ctx.store).await {
        Ok(sessions) => {
            for summary in sessions {
                if let Some(id) = summary["session_id"].as_str() {
                    ctx.sessions.insert(
                        id.to_owned(),
                        Session {
                            summary,
                            rollouts: Vec::new(),
                        },
                    );
                }
            }
        }
        Err(error) => ctx
            .warnings
            .push(format!("state database unavailable: {error}")),
    }
    let mut paths = Vec::new();
    for dir in ["sessions", "archived_sessions"] {
        let root = ctx.store.root.join(dir);
        if !root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    ctx.warnings.push(error.to_string());
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                match files::allowed_path(&ctx.store.root, entry.path()) {
                    Ok(path) => paths.push((path, dir == "archived_sessions")),
                    Err(error) => ctx.warnings.push(error.to_string()),
                }
            }
            if paths.len() > 100_000 {
                return Err(Error::new(
                    "limit_exceeded",
                    "too many rollout files in one store",
                ));
            }
        }
    }
    paths.sort();
    for (path, archived) in paths {
        let mut summary = match normalize::rollout_metadata(&ctx.store.id, &path) {
            Ok(summary) => summary,
            Err(error) => {
                ctx.warnings.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        if summary.get("archived").is_none() {
            summary["archived"] = json!(archived);
        }
        let source = if archived {
            "archived_sessions/**/*.jsonl"
        } else {
            "sessions/**/*.jsonl"
        };
        if !ctx.sources.iter().any(|entry| entry == source) {
            ctx.sources.push(source.to_owned());
        }
        let id = summary["session_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if let Some(existing) = ctx.sessions.get_mut(&id) {
            for (key, value) in summary.as_object().into_iter().flatten() {
                if existing.summary.get(key).is_none_or(Value::is_null) {
                    existing.summary[key] = value.clone();
                }
            }
            if existing.summary["source_kind"] == "sqlite" {
                existing.summary["source_kind"] = json!("sqlite+rollout");
            }
            if updated(&summary) > updated(&existing.summary) {
                existing.summary["updated_at_ms"] = summary["updated_at_ms"].clone();
            }
            existing.rollouts.push(path);
        } else {
            ctx.sessions.insert(
                id,
                Session {
                    summary,
                    rollouts: vec![path],
                },
            );
        }
    }
    let history = ctx.store.root.join("history.jsonl");
    if history.is_file() {
        let mut history_summaries = BTreeMap::<String, Value>::new();
        match files::allowed_path(&ctx.store.root,&history).and_then(|path|files::visit_records(&path,|_,record|{
            let (Some(id),Some(_))=(record["session_id"].as_str(),record["text"].as_str())else{return true;};
            if ctx.sessions.contains_key(id){return true;}
            let time=normalize::timestamp_ms(&record["ts"]);
            let summary=history_summaries.entry(id.to_owned()).or_insert_with(||json!({"store_id":ctx.store.id,"session_id":id,"ref":format!("{}:{id}",ctx.store.id),"source_kind":"history","created_at_ms":time,"updated_at_ms":time,"item_count":0}));
            summary["item_count"]=json!(summary["item_count"].as_u64().unwrap_or(0)+1);
            if let Some(time)=time {
                summary["created_at_ms"]=json!(summary["created_at_ms"].as_i64().map_or(time,|old|old.min(time)));
                summary["updated_at_ms"]=json!(summary["updated_at_ms"].as_i64().map_or(time,|old|old.max(time)));
            }
            true
        })) {
            Ok(())=>{
                ctx.history=Some(history);
                ctx.sessions.extend(history_summaries.into_iter().map(|(id,summary)|(id,Session{summary:normalize::clean(summary),rollouts:Vec::new()})));
            }
            Err(error)=>ctx.warnings.push(format!("prompt history unavailable: {error}")),
        }
    }
    Ok(ctx)
}

async fn visit_items(
    ctx: &Context,
    session: &Session,
    include_tools: bool,
    visit: &mut dyn FnMut(Value) -> bool,
) -> Result<&'static str> {
    let id = session.summary["session_id"].as_str().unwrap_or_default();
    if sqlite::has_items(&ctx.store, id).await? {
        sqlite::visit_items(&ctx.store, id, include_tools, visit).await?;
        return Ok("sqlite_projection");
    }
    if !session.rollouts.is_empty() {
        let mut keep_going = true;
        for path in &session.rollouts {
            let path = files::allowed_path(&ctx.store.root, path)?;
            normalize::visit_rollout(&ctx.store.id, id, &path, include_tools, &mut |item| {
                keep_going = visit(item);
                keep_going
            })?;
            if !keep_going {
                break;
            }
        }
        return Ok(if session.summary["source_kind"] == "openai_export" {
            "openai_export"
        } else {
            "rollout"
        });
    }
    if let Some(history) = &ctx.history {
        let path = files::allowed_path(&ctx.store.root, history)?;
        files::visit_records(&path, |ordinal, record| {
            if record["session_id"] != id {
                return true;
            }
            normalize::history_item(&ctx.store.id, ordinal, &record).is_none_or(&mut *visit)
        })?;
    }
    Ok("history")
}

fn fold(text: &str) -> String {
    unicase::UniCase::new(text).to_folded_case()
}

fn excerpt(text: &str, folded_query: &str) -> Option<String> {
    let position = fold(text).find(folded_query)?;
    let mut folded_bytes = 0;
    let mut original_position = 0;
    for (index, ch) in text.chars().enumerate() {
        let width = fold(&ch.to_string()).len();
        if folded_bytes + width > position {
            original_position = index;
            break;
        }
        folded_bytes += width;
    }
    Some(
        text.chars()
            .skip(original_position.saturating_sub(100))
            .take((folded_query.chars().count() + 240).min(640))
            .collect(),
    )
}

#[cfg(test)]
mod search_tests {
    use super::*;
    #[test]
    fn case_folding_keeps_excerpts_aligned_with_original_unicode() {
        assert_eq!(fold("Straße"), fold("STRASSE"));
        assert_eq!(fold("Σςσ"), "σσσ");
        let text = format!("{} needle here", "ß".repeat(300));
        assert!(
            excerpt(&text, "needle")
                .expect("match")
                .contains("needle here")
        );
    }
}
