use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt, service::RequestContext, tool,
    tool_handler, tool_router,
};
use serde::Serialize;

use crate::Error;
use anlg_agent_access as access;

/// Runtime options of the stdio server.
#[derive(Debug, Clone)]
pub struct McpOptions {
    /// Whether the proposal and recording-import tools may write. The database
    /// connection is read-only when false.
    pub writable: bool,
    /// Ollama base URL for `preview_enhanced_notes`; must be loopback.
    #[cfg_attr(not(feature = "ollama"), allow(dead_code))]
    pub ollama_url: String,
    pub default_model: String,
}

impl McpOptions {
    /// `ANARLOG_OLLAMA_URL` (or Ollama's own `OLLAMA_HOST`) and
    /// `ANARLOG_OLLAMA_MODEL` override the defaults.
    pub fn from_env(writable: bool) -> Self {
        let from_env = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        Self {
            writable,
            ollama_url: from_env("ANARLOG_OLLAMA_URL")
                .or_else(|| {
                    from_env("OLLAMA_HOST").map(|host| crate::preview::ollama_host_to_url(&host))
                })
                .unwrap_or_else(|| crate::preview::DEFAULT_OLLAMA_URL.to_string()),
            default_model: from_env("ANARLOG_OLLAMA_MODEL")
                .unwrap_or_else(|| crate::preview::DEFAULT_MODEL.to_string()),
        }
    }
}

#[derive(Clone)]
struct AnarlogMcpServer {
    db: Arc<anlg_db_core::Db>,
    options: Arc<McpOptions>,
}

#[derive(Debug, PartialEq, Eq)]
enum ResourceRequest {
    Meeting {
        meeting_id: String,
    },
    Transcript {
        meeting_id: String,
        offset: u32,
        limit: u32,
    },
    Series {
        series_id: String,
    },
}

impl AnarlogMcpServer {
    fn new(db: Arc<anlg_db_core::Db>, options: McpOptions) -> Self {
        Self {
            db,
            options: Arc::new(options),
        }
    }

    fn require_writes(&self, tool: &str) -> std::result::Result<(), McpError> {
        if self.options.writable {
            Ok(())
        } else {
            Err(McpError::invalid_request(
                format!(
                    "{tool} writes to the Anarlog database but this server is read-only; register it with `anarlog mcp --allow-writes` to enable it"
                ),
                None,
            ))
        }
    }
}

#[tool_router]
impl AnarlogMcpServer {
    #[tool(
        title = "Import recording",
        description = "WRITES (requires `anarlog mcp --allow-writes`). Create a meeting from a local audio file using the running Anarlog desktop app and its configured providers. May send audio to the configured transcription provider and use paid quota. file_path is on this computer, not a remote host. Returns durable job and meeting IDs. Reuse request_key on retries; poll get_recording_import until done or failed.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::RecordingImport>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn import_recording(
        &self,
        Parameters(input): Parameters<access::ImportRecordingInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        self.require_writes("import_recording")?;
        structured(
            &access::import_recording(self.db.pool(), input)
                .await
                .map_err(command_error)?,
        )
    }

    #[tool(
        title = "Get recording import",
        description = "Get durable recording import processing_status, summary_status and error. A failed summary preserves the meeting and transcript. queued requires the desktop app to run.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::RecordingImport>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn get_recording_import(
        &self,
        Parameters(input): Parameters<access::GetRecordingImportInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        structured(
            &access::get_recording_import(self.db.pool(), &input.job_id)
                .await
                .map_err(command_error)?,
        )
    }

    #[tool(
        title = "List meetings",
        description = "List recent Anarlog meetings with pagination metadata. Use query to narrow by title or meeting id and folder_path to scope to a folder and its subfolders, then pass next_offset as offset to continue.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::MeetingPage>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_meetings(
        &self,
        Parameters(input): Parameters<access::ListMeetingsInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::list_meetings(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }

    #[tool(
        title = "List folders",
        description = "List Anarlog meeting folders, including empty and parent folders, with pagination metadata. Pass a returned path as list_meetings folder_path. Query matches a case-insensitive path substring.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::FolderPage>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_folders(
        &self,
        Parameters(input): Parameters<access::ListFoldersInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::list_folders(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }

    #[tool(
        title = "Get meeting",
        description = "Get one Anarlog meeting with its canonical note, summaries, participants, and action items. Use get_meeting_transcript separately for transcript words.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::LocalMeeting>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_meeting(
        &self,
        Parameters(input): Parameters<access::GetMeetingInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let meeting = access::get_meeting(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        let meeting = access::local_meeting(self.db.pool(), meeting)
            .await
            .map_err(command_error)?;
        structured(&meeting)
    }

    #[tool(
        title = "Get meeting transcript",
        description = "Get a bounded page of transcript words and readable text for an Anarlog meeting. Pass pagination.next_offset as offset to continue.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::TranscriptPage>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_meeting_transcript(
        &self,
        Parameters(input): Parameters<access::GetMeetingTranscriptInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::get_meeting_transcript(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }

    #[tool(
        title = "Get recurring meeting history",
        description = "List meetings in the same recurring series as the supplied meeting, newest first, with pagination metadata.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::MeetingPage>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_recurring_meeting_history(
        &self,
        Parameters(input): Parameters<access::GetRecurringMeetingHistoryInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::get_recurring_meeting_history(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }

    #[tool(
        title = "Export meeting",
        description = "Get a complete Anarlog meeting export with notes, summaries, participants, action items, and transcripts.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::MeetingExport>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn export_meeting(
        &self,
        Parameters(input): Parameters<access::GetMeetingInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let export = access::get_meeting_export(self.db.pool(), input.meeting_id)
            .await
            .map_err(command_error)?;
        structured(&export)
    }

    #[tool(
        title = "Search meetings",
        description = "Full-text search over this computer's local Anarlog database: meeting titles, typed notes, enhanced notes, summaries and transcript text. Every word of the query must appear somewhere in a meeting; wrap a phrase in double quotes. Returns meeting id, title, date and snippets (transcript snippets carry an mm:ss time), newest first, paginated. Read-only; no network and no model.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::SearchPage>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search_meetings(
        &self,
        Parameters(input): Parameters<access::SearchMeetingsInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::search_meetings(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }

    #[tool(
        title = "Get enhanced notes",
        description = "Read a meeting's saved enhanced notes from the local database as structured data: the user's own typed lines in order, each with its AI bullets, and each bullet's resolved sources (speaker, start/end time, the cited transcript words), plus decisions, action items (owner and due when recorded) and open questions. Also returns a Markdown rendering with [^n] footnotes whose definitions start with [mm:ss]. Read-only; no network and no model. Fails with not found when the meeting has no enhanced notes.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::EnhancedNotes>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_enhanced_notes(
        &self,
        Parameters(input): Parameters<access::GetEnhancedNotesInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let notes = access::get_enhanced_notes(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&notes)
    }

    #[tool(
        title = "Get transcript segment",
        description = "Read the transcript text for a source reference (transcript_id plus start_word_id and end_word_id, as cited by get_enhanced_notes) or for a time range (start_ms and end_ms as stored on transcript words). Returns speaker turns with speaker names and mm:ss times, and a ready-to-read text block. Read-only; reads only the local database; no network and no model.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::TranscriptSegment>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_transcript_segment(
        &self,
        Parameters(input): Parameters<access::GetTranscriptSegmentInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let segment = access::get_transcript_segment(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&segment)
    }

    #[tool(
        title = "Preview enhanced notes",
        description = "CALLS THE LOCAL MODEL. Runs the enhanced-notes pass for one meeting with the Ollama model on this computer (loopback only; any non-loopback Ollama URL is refused) and returns the result in the same shape as get_enhanced_notes with origin \"preview\". NOTHING IS SAVED: the database is not written and the desktop app's notes are untouched. It can take minutes on long meetings. To keep a result, a human applies it in the desktop app; you may stage the markdown with propose_summary_edit, which also stays pending until a human applies it.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<PreviewEnhancedNotes>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn preview_enhanced_notes(
        &self,
        Parameters(input): Parameters<PreviewEnhancedNotesInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let model = input
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .unwrap_or(&self.options.default_model)
            .to_string();
        let result = self.run_preview(&input.meeting_id, &model).await?;
        structured(&result)
    }

    #[tool(
        title = "Propose summary edit",
        description = "WRITES a pending proposal row (requires `anarlog mcp --allow-writes`); never edits the meeting itself. Propose a complete summary replacement. The proposal stays pending until a human applies it in the Anarlog desktop app. Specify target_id when the meeting has multiple summaries.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::Proposal>(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn propose_summary_edit(
        &self,
        Parameters(input): Parameters<ProposeSummaryInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        self.require_writes("propose_summary_edit")?;
        let proposal = access::create_proposal(
            self.db.pool(),
            access::CreateProposalInput {
                meeting_id: input.meeting_id,
                kind: "summary_replace".to_string(),
                target_id: input.target_id,
                content: input.content,
                source: Some("mcp".to_string()),
            },
        )
        .await
        .map_err(command_error)?;
        structured(&proposal)
    }

    #[tool(
        title = "Propose memo edit",
        description = "WRITES a pending proposal row (requires `anarlog mcp --allow-writes`); never edits the meeting itself. Propose a complete memo replacement. The proposal stays pending until a human applies it in the Anarlog desktop app.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::Proposal>(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn propose_memo_edit(
        &self,
        Parameters(input): Parameters<ProposeMemoInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        self.require_writes("propose_memo_edit")?;
        let proposal = access::create_proposal(
            self.db.pool(),
            access::CreateProposalInput {
                meeting_id: input.meeting_id,
                kind: "memo_replace".to_string(),
                target_id: None,
                content: input.content,
                source: Some("mcp".to_string()),
            },
        )
        .await
        .map_err(command_error)?;
        structured(&proposal)
    }

    #[tool(
        title = "List proposals",
        description = "List staged Anarlog meeting proposals. Defaults to pending proposals. Pass status all to include applied and declined rows, and next_offset as offset to continue.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::ProposalPage>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_proposals(
        &self,
        Parameters(input): Parameters<access::ListProposalsInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::list_proposals(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }

    #[tool(
        title = "Get proposal",
        description = "Get one staged Anarlog proposal, including its unified diff. The proposal is not applied.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::Proposal>(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_proposal(
        &self,
        Parameters(input): Parameters<access::GetProposalInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let proposal = access::get_proposal(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&proposal)
    }

    #[tool(
        title = "Decline proposal",
        description = "WRITES (requires `anarlog mcp --allow-writes`). Decline a pending proposal without changing the meeting. Applied proposals cannot be declined.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<access::Proposal>(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn decline_proposal(
        &self,
        Parameters(input): Parameters<access::DeclineProposalInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        self.require_writes("decline_proposal")?;
        let proposal = access::decline_proposal(self.db.pool(), input)
            .await
            .map_err(command_error)?;
        structured(&proposal)
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
struct PreviewEnhancedNotesInput {
    /// Anarlog meeting id
    meeting_id: String,
    /// Ollama model name; defaults to the configured notes model
    model: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
struct PreviewEnhancedNotes {
    /// Always false: previews are never written to the database.
    saved: bool,
    model: String,
    /// Problems worth knowing about, such as dropped unsupported bullets.
    warnings: Vec<String>,
    notes: access::EnhancedNotes,
}

impl AnarlogMcpServer {
    #[cfg(feature = "ollama")]
    async fn run_preview(
        &self,
        meeting_id: &str,
        model: &str,
    ) -> std::result::Result<PreviewEnhancedNotes, McpError> {
        use crate::preview::{EnhancedNotesBackend, PreviewBackend};

        // Refuse a non-loopback URL before reading or sending anything.
        let backend = EnhancedNotesBackend::new(&self.options.ollama_url).map_err(preview_error)?;
        let inputs = access::load_enhance_inputs(self.db.pool(), meeting_id)
            .await
            .map_err(command_error)?;
        let run = backend
            .preview(&inputs, model)
            .await
            .map_err(preview_error)?;
        let notes = access::assemble_preview_from_doc(&inputs, &run.doc, run.generation);
        Ok(PreviewEnhancedNotes {
            saved: false,
            model: model.to_string(),
            warnings: run.warnings,
            notes,
        })
    }

    #[cfg(not(feature = "ollama"))]
    async fn run_preview(
        &self,
        _meeting_id: &str,
        _model: &str,
    ) -> std::result::Result<PreviewEnhancedNotes, McpError> {
        Err(McpError::invalid_request(
            "this anarlog build was compiled without the `ollama` feature, so preview_enhanced_notes is unavailable",
            None,
        ))
    }
}

#[cfg(feature = "ollama")]
fn preview_error(error: crate::preview::PreviewError) -> McpError {
    use crate::preview::PreviewError;
    match error {
        PreviewError::NotLoopback(_) | PreviewError::Invalid(_) => {
            McpError::invalid_params(error.to_string(), None)
        }
        PreviewError::Backend(_) => McpError::internal_error(error.to_string(), None),
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
struct ProposeSummaryInput {
    meeting_id: String,
    content: String,
    target_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
struct ProposeMemoInput {
    meeting_id: String,
    content: String,
}

#[tool_handler]
impl ServerHandler for AnarlogMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
        .with_server_info(Implementation::new("anarlog", crate::VERSION))
        .with_instructions(
            "Local access to Anarlog meeting data. Start with list_meetings to resolve a meeting_id; to scope work to a project or client, call list_folders and pass a returned path as folder_path. Then call get_meeting for notes, summaries, participants, and action items. Request transcript pages with get_meeting_transcript and continue with pagination.next_offset; each page is capped at 500 words. Use get_recurring_meeting_history for series context. Use search_meetings to find meetings by words in titles, notes or transcripts. get_enhanced_notes returns the saved enhanced notes with cited sources, and get_transcript_segment reads the transcript behind a citation or a time range. preview_enhanced_notes runs the local Ollama model and returns an unsaved result. Everything runs on this computer. Use export_meeting only when the task needs the complete record including transcripts. This server is read-only unless it was started with --allow-writes. To persist an edit, call propose_summary_edit or propose_memo_edit; the result stays pending until a human applies it in the desktop app. List or inspect staged work with list_proposals and get_proposal. decline_proposal discards a pending proposal without changing the meeting. Never invent meeting titles, dates, or ids. If list_meetings returns no meetings, say so. Never access SQLite directly, or claim a proposal was applied. Documentation: https://docs.anarlog.so",
        )
    }

    async fn list_resources(
        &self,
        params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourcesResult, McpError> {
        let offset = params
            .and_then(|params| params.cursor)
            .map(|cursor| {
                cursor.parse::<u32>().map_err(|_| {
                    McpError::invalid_params("resource cursor must be an integer", None)
                })
            })
            .transpose()?
            .unwrap_or(0);
        let page = access::list_meetings(
            self.db.pool(),
            access::ListMeetingsInput {
                query: None,
                series_id: None,
                folder_path: None,
                limit: Some(access::DEFAULT_LIST_LIMIT),
                offset: Some(offset),
            },
        )
        .await
        .map_err(command_error)?;
        let next_cursor = page.pagination.next_offset.map(|offset| offset.to_string());
        let resources = page
            .meetings
            .into_iter()
            .map(|meeting| {
                let name = if meeting.title.trim().is_empty() {
                    "Untitled meeting".to_string()
                } else {
                    meeting.title
                };
                Resource::new(format!("anarlog://meetings/{}", meeting.id), name)
                    .with_description("Anarlog meeting context")
                    .with_mime_type("text/markdown")
            })
            .collect();

        let mut result = ListResourcesResult::with_all_items(resources);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    async fn list_resource_templates(
        &self,
        _params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("anarlog://meetings/{meeting_id}", "Anarlog meeting")
                .with_description("Meeting metadata, note, summaries, people, and action items")
                .with_mime_type("text/markdown"),
            ResourceTemplate::new(
                "anarlog://meetings/{meeting_id}/transcript{?offset,limit}",
                "Anarlog meeting transcript",
            )
            .with_description("A bounded page of meeting transcript text")
            .with_mime_type("text/plain"),
            ResourceTemplate::new("anarlog://series/{series_id}", "Anarlog meeting series")
                .with_description("Recurring meeting history")
                .with_mime_type("text/markdown"),
        ]))
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ReadResourceResponse, McpError> {
        let request = parse_resource_uri(&params.uri)?;
        let contents = match request {
            ResourceRequest::Meeting { meeting_id } => {
                let meeting =
                    access::get_meeting(self.db.pool(), access::GetMeetingInput { meeting_id })
                        .await
                        .map_err(command_error)?;
                ResourceContents::text(meeting.to_markdown(), params.uri)
                    .with_mime_type("text/markdown")
            }
            ResourceRequest::Transcript {
                meeting_id,
                offset,
                limit,
            } => {
                let page = access::get_meeting_transcript(
                    self.db.pool(),
                    access::GetMeetingTranscriptInput {
                        meeting_id,
                        offset: Some(offset),
                        limit: Some(limit),
                    },
                )
                .await
                .map_err(command_error)?;
                ResourceContents::text(page.text, params.uri).with_mime_type("text/plain")
            }
            ResourceRequest::Series { series_id } => {
                let page = access::list_meetings(
                    self.db.pool(),
                    access::ListMeetingsInput {
                        query: None,
                        series_id: Some(series_id),
                        folder_path: None,
                        limit: Some(100),
                        offset: Some(0),
                    },
                )
                .await
                .map_err(command_error)?;
                let text = page
                    .meetings
                    .into_iter()
                    .map(|meeting| {
                        let title = if meeting.title.is_empty() {
                            "Untitled"
                        } else {
                            &meeting.title
                        };
                        let date = if meeting.started_at.is_empty() {
                            &meeting.created_at
                        } else {
                            &meeting.started_at
                        };
                        format!("- {date} — [{title}](anarlog://meetings/{})", meeting.id)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                ResourceContents::text(text, params.uri).with_mime_type("text/markdown")
            }
        };

        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

/// Serve MCP over stdin/stdout. This is the only transport: the process never
/// binds a socket or opens a network listener, in local-only builds or any other.
pub async fn serve(db: Arc<anlg_db_core::Db>, options: McpOptions) -> crate::Result<()> {
    let running = AnarlogMcpServer::new(db, options)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| Error::operation("start MCP server", error.to_string()))?;
    running
        .waiting()
        .await
        .map_err(|error| Error::operation("run MCP server", error.to_string()))?;
    Ok(())
}

fn parse_resource_uri(uri: &str) -> std::result::Result<ResourceRequest, McpError> {
    let url = url::Url::parse(uri)
        .map_err(|_| McpError::invalid_params("invalid Anarlog resource URI", None))?;
    if url.scheme() != "anarlog" {
        return Err(McpError::invalid_params(
            "resource URI must use the anarlog scheme",
            None,
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| McpError::invalid_params("resource URI is missing a type", None))?;
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    match (host, segments.as_slice()) {
        ("meetings", [meeting_id]) => Ok(ResourceRequest::Meeting {
            meeting_id: (*meeting_id).to_string(),
        }),
        ("meetings", [meeting_id, "transcript"]) => {
            let mut offset = 0;
            let mut limit = access::DEFAULT_TRANSCRIPT_LIMIT;
            for (key, value) in url.query_pairs() {
                match key.as_ref() {
                    "offset" => {
                        offset = value.parse().map_err(|_| {
                            McpError::invalid_params("transcript offset must be an integer", None)
                        })?;
                    }
                    "limit" => {
                        limit = value.parse::<u32>().map_err(|_| {
                            McpError::invalid_params("transcript limit must be an integer", None)
                        })?;
                    }
                    _ => {}
                }
            }
            Ok(ResourceRequest::Transcript {
                meeting_id: (*meeting_id).to_string(),
                offset,
                limit: limit.clamp(1, access::MAX_TRANSCRIPT_LIMIT),
            })
        }
        ("series", [series_id]) => Ok(ResourceRequest::Series {
            series_id: (*series_id).to_string(),
        }),
        _ => Err(McpError::invalid_params(
            "unsupported Anarlog resource URI",
            None,
        )),
    }
}

fn structured(value: &impl Serialize) -> std::result::Result<CallToolResult, McpError> {
    serde_json::to_value(value)
        .map(CallToolResult::structured)
        .map_err(internal_error)
}

fn internal_error(error: impl std::fmt::Display) -> McpError {
    McpError::internal_error(error.to_string(), None)
}

fn command_error(error: access::Error) -> McpError {
    match error {
        access::Error::NotFound(what) => {
            McpError::invalid_params(format!("{what} not found"), None)
        }
        access::Error::Invalid(reason) => McpError::invalid_params(reason, None),
        other => internal_error(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::transport::{IntoTransport, Transport};
    use serde_json::Value;

    #[test]
    fn parses_supported_resource_uris_and_bounds_transcript_limit() {
        assert_eq!(
            parse_resource_uri("anarlog://meetings/meeting-1").unwrap(),
            ResourceRequest::Meeting {
                meeting_id: "meeting-1".to_string()
            }
        );
        assert_eq!(
            parse_resource_uri("anarlog://meetings/meeting-1/transcript?offset=4&limit=900")
                .unwrap(),
            ResourceRequest::Transcript {
                meeting_id: "meeting-1".to_string(),
                offset: 4,
                limit: access::MAX_TRANSCRIPT_LIMIT,
            }
        );
        assert!(parse_resource_uri("file:///tmp/meeting").is_err());
    }

    #[tokio::test]
    async fn list_tool_returns_structured_meeting_data() {
        let db = anlg_db_core::Db::connect_memory_plain().await.unwrap();
        anlg_db_app::prepare_schema(&db).await.unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, title, started_at, folder_path)
             VALUES
             ('meeting-1', 'Planning', '2026-07-13', 'Clients/ACME'),
             ('meeting-2', 'Planning retro', '2026-07-12', '')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let server = AnarlogMcpServer::new(Arc::new(db), test_options(true));

        let result = server
            .list_meetings(Parameters(access::ListMeetingsInput {
                query: Some("plan".to_string()),
                series_id: None,
                folder_path: Some("Clients".to_string()),
                limit: None,
                offset: None,
            }))
            .await
            .unwrap();

        let meetings = result.structured_content.unwrap();
        assert_eq!(meetings["meetings"][0]["id"], "meeting-1");
        assert_eq!(meetings["meetings"][0]["title"], "Planning");
        assert_eq!(meetings["meetings"][0]["folder_path"], "Clients/ACME");
        assert_eq!(meetings["pagination"]["returned"], 1);
        assert!(meetings["pagination"]["next_offset"].is_null());

        let unfiled = server
            .get_meeting(Parameters(access::GetMeetingInput {
                meeting_id: "meeting-2".to_string(),
            }))
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert!(unfiled["folder_path"].is_null());

        let folders = server
            .list_folders(Parameters(access::ListFoldersInput::default()))
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(folders["folders"][1]["path"], "Clients/ACME");
        assert_eq!(folders["folders"][1]["name"], "ACME");
        assert_eq!(folders["folders"][1]["parent_path"], "Clients");
        assert!(folders["folders"][0]["parent_path"].is_null());

        let invalid = server
            .list_meetings(Parameters(access::ListMeetingsInput {
                folder_path: Some("/Clients".to_string()),
                ..Default::default()
            }))
            .await;
        assert!(invalid.is_err());
    }

    #[tokio::test]
    async fn client_server_handshake_lists_tools_and_resources() {
        let db = anlg_db_core::Db::connect_memory_plain().await.unwrap();
        anlg_db_app::prepare_schema(&db).await.unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, title, started_at) VALUES ('meeting-1', 'Planning', '2026-07-13')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server = AnarlogMcpServer::new(Arc::new(db), test_options(true));
        let info = server.get_info();
        let server_handle = tokio::spawn(async move { server.serve(server_transport).await });

        let client = ().serve(client_transport).await.unwrap();
        let tools = client.list_all_tools().await.unwrap();
        let templates = client.list_all_resource_templates().await.unwrap();
        let resources = client.list_all_resources().await.unwrap();
        insta::assert_json_snapshot!(
            "mcp_contract",
            canonicalize_json(serde_json::json!({
                "protocol_version": info.protocol_version,
                "instructions": info.instructions,
                "tools": tools,
                "resource_templates": templates,
            }))
        );

        let mut tool_names = tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        tool_names.sort();
        assert_eq!(
            tool_names,
            [
                "decline_proposal",
                "export_meeting",
                "get_enhanced_notes",
                "get_meeting",
                "get_meeting_transcript",
                "get_proposal",
                "get_recording_import",
                "get_recurring_meeting_history",
                "get_transcript_segment",
                "import_recording",
                "list_folders",
                "list_meetings",
                "list_proposals",
                "preview_enhanced_notes",
                "propose_memo_edit",
                "propose_summary_edit",
                "search_meetings",
            ]
        );
        let mcp_docs = include_str!("../../../docs/reference/mcp.mdx");
        let mcp_skill = include_str!("../../../skills/anarlog/references/mcp.md");
        // Tools added by this fork are documented in docs-local/MCP.md.
        let fork_docs = include_str!("../../../docs-local/MCP.md");
        const FORK_TOOLS: [&str; 4] = [
            "search_meetings",
            "get_enhanced_notes",
            "get_transcript_segment",
            "preview_enhanced_notes",
        ];
        for tool_name in &tool_names {
            if FORK_TOOLS.contains(&tool_name.as_str()) {
                assert!(
                    fork_docs.contains(tool_name),
                    "docs-local/MCP.md is missing `{tool_name}`"
                );
                continue;
            }
            assert!(
                mcp_docs.contains(tool_name),
                "MCP docs are missing `{tool_name}`"
            );
            assert!(
                mcp_skill.contains(tool_name),
                "Anarlog skill is missing `{tool_name}`"
            );
        }
        for tool in tools {
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .expect("tool input properties");
            let docs = if FORK_TOOLS.contains(&tool.name.as_ref()) {
                fork_docs
            } else {
                mcp_docs
            };
            for parameter in properties.keys() {
                assert!(
                    docs.contains(&format!("`{parameter}`")),
                    "docs are missing `{parameter}` of {}",
                    tool.name
                );
            }
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(
                FORK_TOOLS.contains(&tool.name.as_ref())
                    || !description.contains("WRITES")
                    || description.contains("--allow-writes"),
                "{} must say how to enable writes",
                tool.name
            );
            assert_eq!(
                tool.output_schema
                    .as_ref()
                    .and_then(|schema| schema.get("type"))
                    .and_then(Value::as_str),
                Some("object")
            );
            let annotations = tool.annotations.expect("tool annotations");
            let write_tool = matches!(
                tool.name.as_ref(),
                "propose_summary_edit"
                    | "propose_memo_edit"
                    | "decline_proposal"
                    | "import_recording"
            );
            assert_eq!(annotations.read_only_hint, Some(!write_tool));
            assert_eq!(annotations.destructive_hint, Some(false));
            // The preview calls a sampling model, so repeated calls differ.
            assert_eq!(
                annotations.idempotent_hint,
                Some(!write_tool && tool.name != "preview_enhanced_notes")
            );
            assert_eq!(
                annotations.open_world_hint,
                Some(tool.name == "import_recording")
            );
        }

        let mut template_contract = templates
            .iter()
            .map(|template| {
                (
                    template.name.clone(),
                    template.uri_template.clone(),
                    template.annotations.clone(),
                )
            })
            .collect::<Vec<_>>();
        template_contract.sort_by(|left, right| left.1.cmp(&right.1));
        assert_eq!(
            template_contract,
            [
                (
                    "Anarlog meeting".to_string(),
                    "anarlog://meetings/{meeting_id}".to_string(),
                    None,
                ),
                (
                    "Anarlog meeting transcript".to_string(),
                    "anarlog://meetings/{meeting_id}/transcript{?offset,limit}".to_string(),
                    None,
                ),
                (
                    "Anarlog meeting series".to_string(),
                    "anarlog://series/{series_id}".to_string(),
                    None,
                ),
            ]
        );
        for (_, uri, _) in &template_contract {
            assert!(mcp_docs.contains(uri), "MCP docs are missing `{uri}`");
            assert!(mcp_skill.contains(uri), "Anarlog skill is missing `{uri}`");
        }
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].name, "Planning");
        assert_eq!(resources[0].uri, "anarlog://meetings/meeting-1");
        assert!(resources[0].annotations.is_none());

        client.cancel().await.unwrap();
        let server = server_handle.await.unwrap().unwrap();
        server.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn current_client_discovers_and_lists_tools_without_initialize() {
        let db = Arc::new(anlg_db_core::Db::connect_memory_plain().await.unwrap());
        anlg_db_app::prepare_schema(&db).await.unwrap();
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server = AnarlogMcpServer::new(db, test_options(false));
        let server_task =
            tokio::spawn(async move { server.serve(server_transport).await.unwrap() });
        let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_transport);

        let mut discover = DiscoverRequest::new(DiscoverRequestParams {});
        discover.extensions.insert(current_client_meta());
        client
            .send(ClientJsonRpcMessage::request(
                ClientRequest::DiscoverRequest(discover),
                RequestId::Number(1),
            ))
            .await
            .unwrap();
        let discover = client.receive().await.unwrap();
        let discover = serde_json::to_value(discover).unwrap();
        assert!(
            discover["result"]["supportedVersions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|version| version == "2026-07-28")
        );

        let mut list_tools = ListToolsRequest {
            method: Default::default(),
            params: None,
            extensions: Default::default(),
        };
        list_tools.extensions.insert(current_client_meta());
        client
            .send(ClientJsonRpcMessage::request(
                ClientRequest::ListToolsRequest(list_tools),
                RequestId::Number(2),
            ))
            .await
            .unwrap();
        let tools = client.receive().await.unwrap();
        let tools = serde_json::to_value(tools).unwrap();
        assert_eq!(tools["result"]["resultType"], "complete");
        assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 17);

        drop(client);
        server_task.await.unwrap().cancel().await.unwrap();
    }

    async fn meeting_db() -> Arc<anlg_db_core::Db> {
        let db = anlg_db_core::Db::connect_memory_plain().await.unwrap();
        anlg_db_app::prepare_schema(&db).await.unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, title, started_at) VALUES ('m1', 'Launch planning', '2026-07-13')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let words = serde_json::json!([
            {"id": "w1", "text": " We", "start_ms": 0, "end_ms": 300, "channel": 0},
            {"id": "w2", "text": " ship", "start_ms": 300, "end_ms": 600, "channel": 0},
            {"id": "w3", "text": " Friday.", "start_ms": 600, "end_ms": 1000, "channel": 0},
            {"id": "w4", "text": " Dana", "start_ms": 65_000, "end_ms": 65_400, "channel": 1},
            {"id": "w5", "text": " owns", "start_ms": 65_400, "end_ms": 65_800, "channel": 1},
            {"id": "w6", "text": " pricing.", "start_ms": 65_800, "end_ms": 66_500, "channel": 1}
        ]);
        sqlx::query("INSERT INTO transcripts (id, session_id, words_json) VALUES ('t1', 'm1', ?)")
            .bind(words.to_string())
            .execute(db.pool())
            .await
            .unwrap();
        let note = serde_json::json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Launch date"}]}
        ]});
        sqlx::query(
            "INSERT INTO session_documents (id, session_id, kind, body_format, body)
             VALUES ('m1', 'm1', 'note', 'prosemirror_json', ?)",
        )
        .bind(note.to_string())
        .execute(db.pool())
        .await
        .unwrap();
        let enhanced = serde_json::json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Launch date"}]},
            {"type": "bulletList", "content": [{"type": "listItem", "attrs": {"aiId": "a1", "src": [
                {"transcript_id": "t1", "start_word_id": "w1", "end_word_id": "w3", "start_ms": 0, "end_ms": 1000}
            ]}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Ship on Friday"}]}]}]}
        ]});
        sqlx::query(
            "INSERT INTO session_documents (id, session_id, kind, body_format, body)
             VALUES ('enh', 'm1', 'summary', 'prosemirror_json', ?)",
        )
        .bind(enhanced.to_string())
        .execute(db.pool())
        .await
        .unwrap();
        Arc::new(db)
    }

    #[tokio::test]
    async fn read_tools_return_structured_results() {
        let server = AnarlogMcpServer::new(meeting_db().await, test_options(false));

        let search = server
            .search_meetings(Parameters(access::SearchMeetingsInput {
                query: "pricing".into(),
                folder_path: None,
                limit: None,
                offset: None,
            }))
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(search["hits"][0]["meeting_id"], "m1");
        assert_eq!(search["hits"][0]["title"], "Launch planning");
        assert_eq!(search["hits"][0]["date"], "2026-07-13");
        assert_eq!(search["hits"][0]["snippets"][0]["source"], "transcript");
        assert_eq!(search["hits"][0]["snippets"][0]["time"], "01:05");

        let enhanced = server
            .get_enhanced_notes(Parameters(access::GetEnhancedNotesInput {
                meeting_id: "m1".into(),
                document_id: None,
            }))
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(enhanced["lines"][0]["text"], "Launch date");
        let bullet = &enhanced["lines"][0]["ai_bullets"][0];
        assert_eq!(bullet["text"], "Ship on Friday");
        assert_eq!(bullet["sources"][0]["text"], "We ship Friday.");
        assert_eq!(bullet["sources"][0]["start"], "00:00");
        assert!(
            enhanced["markdown"]
                .as_str()
                .unwrap()
                .contains("[^1]: [00:00–00:01]")
        );

        let segment = server
            .get_transcript_segment(Parameters(access::GetTranscriptSegmentInput {
                meeting_id: "m1".into(),
                transcript_id: Some("t1".into()),
                start_word_id: Some("w4".into()),
                end_word_id: Some("w6".into()),
                start_ms: None,
                end_ms: None,
                max_words: None,
            }))
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(segment["segments"][0]["text"], "Dana owns pricing.");
        assert_eq!(segment["word_count"], 3);

        let invalid = server
            .get_transcript_segment(Parameters(access::GetTranscriptSegmentInput {
                meeting_id: "m1".into(),
                transcript_id: None,
                start_word_id: None,
                end_word_id: None,
                start_ms: None,
                end_ms: None,
                max_words: None,
            }))
            .await
            .unwrap_err();
        assert_eq!(invalid.code, ErrorCode::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn read_only_server_refuses_write_tools() {
        let server = AnarlogMcpServer::new(meeting_db().await, test_options(false));
        let error = server
            .propose_memo_edit(Parameters(ProposeMemoInput {
                meeting_id: "m1".into(),
                content: "x".into(),
            }))
            .await
            .unwrap_err();
        assert!(error.message.contains("--allow-writes"));
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM session_proposals")
            .fetch_one(server.db.pool())
            .await
            .unwrap();
        assert_eq!(pending, 0);

        let server = AnarlogMcpServer::new(server.db.clone(), test_options(true));
        server
            .propose_memo_edit(Parameters(ProposeMemoInput {
                meeting_id: "m1".into(),
                content: "x".into(),
            }))
            .await
            .unwrap();
    }

    #[cfg(feature = "ollama")]
    #[tokio::test]
    async fn preview_calls_the_local_model_and_saves_nothing() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };

        let model = MockServer::start().await;
        let reply = serde_json::json!({
            "sections": [{"anchor": "a1", "bullets": [
                {"text": "Ship Friday", "sources": ["s1"]},
                {"text": "Made up", "sources": ["s42"]}
            ]}],
            "decisions": [{"text": "Friday launch", "sources": ["s1"]}],
            "action_items": [{"text": "Finish pricing", "owner": "Dana", "due": "Friday", "sources": ["s2"]}],
            "open_questions": []
        });
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {"role": "assistant", "content": reply.to_string()},
                "done": true
            })))
            .expect(1)
            .mount(&model)
            .await;

        let db = meeting_db().await;
        let docs_before: i64 = sqlx::query_scalar("SELECT count(*) FROM session_documents")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let mut options = test_options(false);
        options.ollama_url = model.uri();
        let server = AnarlogMcpServer::new(db.clone(), options);

        let result = server
            .preview_enhanced_notes(Parameters(PreviewEnhancedNotesInput {
                meeting_id: "m1".into(),
                model: Some("stub-model".into()),
            }))
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(result["saved"], false);
        assert_eq!(result["model"], "stub-model");
        assert_eq!(result["notes"]["origin"], "preview");
        assert!(result["notes"]["document_id"].is_null());
        // Generated by the same engine the app saves with.
        assert_eq!(result["notes"]["generation"]["backend"], "enhanced-notes");
        assert_eq!(result["notes"]["generation"]["model"], "stub-model");
        let line = &result["notes"]["lines"][0];
        assert_eq!(line["text"], "Launch date");
        assert_eq!(line["ai_bullets"].as_array().unwrap().len(), 1);
        assert_eq!(
            line["ai_bullets"][0]["sources"][0]["text"],
            "We ship Friday."
        );
        assert_eq!(result["notes"]["decisions"][0]["text"], "Friday launch");
        let action = &result["notes"]["action_items"][0];
        assert_eq!(action["owner"], "Dana");
        assert_eq!(
            action["sources"][0]["speakers"].as_array().unwrap().len(),
            1
        );
        assert!(
            result["warnings"][0]
                .as_str()
                .unwrap()
                .contains("Dropped 1")
        );

        let requests = model.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["model"], "stub-model");
        assert_eq!(body["stream"], false);
        assert!(body["format"]["properties"]["sections"].is_object());
        let prompt = body["messages"][1]["content"].as_str().unwrap();
        assert!(prompt.contains("a1: Launch date"));
        assert!(prompt.contains("s1 ["));

        let docs_after: i64 = sqlx::query_scalar("SELECT count(*) FROM session_documents")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(docs_before, docs_after);
    }

    #[cfg(feature = "ollama")]
    #[tokio::test]
    async fn preview_refuses_non_loopback_ollama_urls() {
        for url in [
            "https://api.openai.com",
            "http://192.168.1.9:11434",
            "0.0.0.0:11434",
        ] {
            let mut options = test_options(false);
            options.ollama_url = url.to_string();
            let server = AnarlogMcpServer::new(meeting_db().await, options);
            let error = server
                .preview_enhanced_notes(Parameters(PreviewEnhancedNotesInput {
                    meeting_id: "m1".into(),
                    model: None,
                }))
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS, "{url}");
            assert!(error.message.contains("loopback"), "{url}");
        }
    }

    fn test_options(writable: bool) -> McpOptions {
        McpOptions {
            writable,
            ollama_url: crate::preview::DEFAULT_OLLAMA_URL.to_string(),
            default_model: "test-model".to_string(),
        }
    }

    fn current_client_meta() -> RequestMetaObject {
        RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::new("anarlog-test", "1.0.0"),
            ClientCapabilities::default(),
        )
    }

    fn canonicalize_json(value: Value) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => {
                Value::Array(values.into_iter().map(canonicalize_json).collect())
            }
            value => value,
        }
    }
}
