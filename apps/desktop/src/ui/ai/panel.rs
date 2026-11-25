use super::{
    client::{self, AiApiStyle, AiConfig, AiMessage, AiProvider},
    conversation::{self, AiChatMessage, AiConversation, MAX_CONVERSATIONS},
    prompts::{self, AiAction, AiContext},
    settings::{AiSettings, API_KEY_LABEL},
    AiEditorContext,
};
use crate::ui::{
    app::style::Palette,
    i18n::{self, Locale},
};
use app_core::metadata::MetadataService;
use eframe::egui::{
    self, Align, Button, ComboBox, Context, Frame, Label, Layout, Margin, RichText, ScrollArea,
    SidePanel, Stroke, TextEdit, Ui,
};
use secure_store::{EncryptedFileStore, SecureStore};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    runtime::Handle,
    sync::{mpsc, Notify},
};

const MAX_CONTEXT_CHARS: usize = 24_000;
const PROMPT_HEIGHT: f32 = 72.0;

enum Event {
    Delta(String, String),
    Finished(Result<(), String>),
}

enum SettingsResult {
    Tested(client::AiTestResult),
    Models(Vec<String>),
    Saved(AiSettings),
}

struct Request {
    events: mpsc::UnboundedReceiver<Event>,
    cancel: Arc<Notify>,
    answer: String,
    reasoning: String,
}

pub struct AiPanel {
    pub open: bool,
    data_dir: PathBuf,
    metadata: MetadataService,
    settings: AiSettings,
    draft: AiSettings,
    api_key: String,
    settings_open: bool,
    conversations: Vec<AiConversation>,
    active: Option<String>,
    input: String,
    action: AiAction,
    include_sql: bool,
    request: Option<Request>,
    settings_job: Option<mpsc::Receiver<Result<SettingsResult, String>>>,
    settings_status: String,
    status: String,
}

fn label(english: &str, chinese: &str) -> String {
    if i18n::locale() == Locale::ZhCn {
        chinese
    } else {
        english
    }
    .to_owned()
}

fn from_setting<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).ok()
}

fn secret_store(data_dir: &std::path::Path) -> Result<EncryptedFileStore, String> {
    let key = crate::config::master_key(data_dir).map_err(|error| error.to_string())?;
    Ok(EncryptedFileStore::new(
        data_dir.join("ai-secrets.json"),
        key,
    ))
}

impl AiPanel {
    pub fn new(data_dir: PathBuf, metadata: MetadataService) -> Self {
        let settings = AiSettings::load(&data_dir);
        let draft = if settings.model.is_empty() {
            AiSettings::default_for_provider("openai")
        } else {
            settings.clone()
        };
        let conversations = conversation::load_conversations(&data_dir);
        let active = conversations.first().map(|item| item.id.clone());
        Self {
            open: false,
            data_dir,
            metadata,
            settings,
            draft,
            api_key: String::new(),
            settings_open: false,
            conversations,
            active,
            input: String::new(),
            action: AiAction::General,
            include_sql: false,
            request: None,
            settings_job: None,
            settings_status: String::new(),
            status: String::new(),
        }
    }

    pub fn show_chat(
        &mut self,
        ctx: &Context,
        runtime: &Handle,
        editor: &AiEditorContext,
        palette: Palette,
    ) -> Option<String> {
        self.poll();
        self.poll_settings_job();
        if !self.open {
            return None;
        }
        let max_width = (ctx.available_rect().width() * 0.56).clamp(270.0, 560.0);
        SidePanel::right("orvyn_ai_chat")
            .default_width(360.0_f32.min(max_width))
            .width_range(270.0..=max_width)
            .resizable(true)
            .frame(
                Frame::side_top_panel(&ctx.style())
                    .fill(palette.bg)
                    .inner_margin(Margin::symmetric(12.0, 10.0)),
            )
            .show(ctx, |ui| {
                self.chat_header(ui, palette, editor);
                ui.separator();
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    let row_height = ui.spacing().interact_size.y;
                    let composer_height = PROMPT_HEIGHT
                        + row_height.max(26.0)
                        + row_height
                        + 3.0 * ui.spacing().item_spacing.y
                        + if self.status.is_empty() {
                            0.0
                        } else {
                            row_height
                        };
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), composer_height),
                        Layout::top_down(Align::Min),
                        |ui| self.composer(ui, runtime, editor, palette),
                    );
                    ui.separator();
                    ui.add_space(8.0);
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        self.transcript(ui, editor, palette)
                    })
                    .inner
                })
                .inner
            })
            .inner
    }

    pub fn show_settings(&mut self, ctx: &Context, runtime: &Handle) {
        if !self.settings_open {
            return;
        }
        let mut open = self.settings_open;
        egui::Window::new(label("AI Settings", "AI 设置"))
            .id(egui::Id::new("orvyn_ai_settings"))
            .open(&mut open)
            .default_width(420.0)
            .min_width(320.0)
            .vscroll(true)
            .show(ctx, |ui| {
                self.settings_ui(ui, runtime);
                if !self.settings_status.is_empty() {
                    ui.separator();
                    ui.label(&self.settings_status);
                }
            });
        self.settings_open = open;
        if !open {
            self.api_key.clear();
        }
    }

    fn settings_ui(&mut self, ui: &mut Ui, runtime: &Handle) {
        let busy = self.settings_job.is_some();
        ui.add_enabled_ui(!busy && self.request.is_none(), |ui| {
            let previous = self.draft.provider.clone();
            let selected = client::provider_presets()
                .iter()
                .find(|item| item.id == previous);
            ComboBox::from_label(label("Provider", "服务商"))
                .selected_text(selected.map_or(previous.as_str(), |item| item.label))
                .show_ui(ui, |ui| {
                    for preset in client::provider_presets() {
                        ui.selectable_value(
                            &mut self.draft.provider,
                            preset.id.to_owned(),
                            preset.label,
                        );
                    }
                });
            if self.draft.provider != previous {
                self.draft = AiSettings::default_for_provider(&self.draft.provider);
                self.api_key.clear();
            }
            ui.label(label("API endpoint", "API 地址"));
            ui.add(TextEdit::singleline(&mut self.draft.endpoint).desired_width(f32::INFINITY));
            ui.label(label("Model", "模型"));
            ui.add(TextEdit::singleline(&mut self.draft.model).desired_width(f32::INFINITY));
            if !self.draft.models.is_empty() {
                ComboBox::from_label(label("Available models", "可用模型"))
                    .selected_text(&self.draft.model)
                    .show_ui(ui, |ui| {
                        for model in &self.draft.models {
                            ui.selectable_value(&mut self.draft.model, model.clone(), model);
                        }
                    });
            }
            if matches!(
                self.draft.provider.as_str(),
                "openai" | "openai-compatible" | "custom"
            ) {
                ComboBox::from_label(label("API format", "API 格式"))
                    .selected_text(&self.draft.api_style)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.draft.api_style,
                            "completions".into(),
                            "Chat Completions",
                        );
                        ui.selectable_value(
                            &mut self.draft.api_style,
                            "responses".into(),
                            "Responses",
                        );
                    });
            }
            ui.label(label("API key", "API 密钥"));
            ui.add(
                TextEdit::singleline(&mut self.api_key)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            if self.draft.credential_ref.is_some() && self.same_endpoint() {
                ui.horizontal(|ui| {
                    ui.weak(label("Saved key", "已保存密钥"));
                    if ui
                        .button(label("Clear saved key", "清除已保存密钥"))
                        .clicked()
                    {
                        self.draft.credential_ref = None;
                    }
                });
            }
            ui.checkbox(
                &mut self.draft.enable_thinking,
                label("Enable reasoning when supported", "启用模型支持的推理模式"),
            );
            ui.horizontal(|ui| {
                if ui.button(label("Test connection", "测试连接")).clicked() {
                    self.start_settings_job(ui.ctx(), runtime, false);
                }
                if ui.button(label("Fetch models", "获取模型")).clicked() {
                    self.start_settings_job(ui.ctx(), runtime, true);
                }
            });
            if self.settings_job.is_none()
                && ui.button(label("Save settings", "保存设置")).clicked()
            {
                self.save_settings(ui.ctx(), runtime);
            }
        });
        if busy {
            ui.spinner();
        }
    }

    fn start_settings_job(&mut self, ctx: &Context, runtime: &Handle, fetch_models: bool) {
        if self.settings_job.is_some() {
            return;
        }
        let mut draft = self.draft.clone();
        if !self.same_endpoint() {
            draft.credential_ref = None;
        }
        let data_dir = self.data_dir.clone();
        let key = self.api_key.trim().to_owned();
        let repaint = ctx.clone();
        let (sender, receiver) = mpsc::channel(1);
        self.settings_job = Some(receiver);
        self.settings_status.clear();
        runtime.spawn(async move {
            let result = async {
                let config = load_config(&data_dir, &draft, key).await?;
                if fetch_models {
                    client::validate_endpoint(&config)?;
                    client::list_models(&config)
                        .await
                        .map(SettingsResult::Models)
                        .map_err(|error| redact_error(error, &config.api_key))
                } else {
                    let mut result = client::test_connection(&config).await;
                    result.message = redact_error(result.message, &config.api_key);
                    Ok(SettingsResult::Tested(result))
                }
            }
            .await;
            let _ = sender.send(result).await;
            repaint.request_repaint();
        });
    }

    fn poll_settings_job(&mut self) {
        let Some(receiver) = &mut self.settings_job else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::error::TryRecvError::Empty) => return,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                Err(label("Request ended unexpectedly.", "请求意外结束。"))
            }
        };
        self.settings_job = None;
        self.settings_status = match result {
            Ok(SettingsResult::Saved(settings)) => {
                self.settings = settings.clone();
                self.draft = settings;
                self.api_key.clear();
                self.settings_open = false;
                String::new()
            }
            Ok(SettingsResult::Tested(result)) if result.ok => format!(
                "{} ({} ms)",
                label("Connection successful", "连接成功"),
                result.latency_ms
            ),
            Ok(SettingsResult::Tested(result)) => {
                format!("{} · {}", result.category, result.message)
            }
            Ok(SettingsResult::Models(models)) if models.is_empty() => {
                label("The provider returned no models.", "服务商未返回模型。")
            }
            Ok(SettingsResult::Models(models)) => {
                let count = models.len();
                if self.draft.model.trim().is_empty() {
                    self.draft.model = models[0].clone();
                }
                self.draft.models = models;
                format!("{} ({count})", label("Models loaded", "已获取模型"))
            }
            Err(error) => error,
        };
    }

    fn same_endpoint(&self) -> bool {
        self.draft.provider == self.settings.provider
            && self.draft.endpoint.trim() == self.settings.endpoint
    }

    fn save_settings(&mut self, ctx: &Context, runtime: &Handle) {
        let mut settings = self.draft.clone();
        settings.endpoint = settings.endpoint.trim().to_owned();
        settings.model = settings.model.trim().to_owned();
        let previous = self.settings.credential_ref.clone();
        if !self.same_endpoint() {
            settings.credential_ref = None;
        }
        let data_dir = self.data_dir.clone();
        let new_key = self.api_key.trim().to_owned();
        let repaint = ctx.clone();
        let (sender, receiver) = mpsc::channel(1);
        self.settings_job = Some(receiver);
        self.settings_status.clear();
        runtime.spawn(async move {
            let result = persist_settings(&data_dir, settings, previous, &new_key)
                .await
                .map(SettingsResult::Saved);
            let _ = sender.send(result).await;
            repaint.request_repaint();
        });
    }

    fn chat_header(&mut self, ui: &mut Ui, palette: Palette, editor: &AiEditorContext) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(label("AI Assistant", "AI 助手"))
                    .size(16.0)
                    .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button("×")
                    .on_hover_text(label("Close chat", "关闭对话"))
                    .clicked()
                {
                    self.open = false;
                }
                if ui.button(label("Settings", "设置")).clicked() {
                    self.settings_open = true;
                }
            });
        });

        let model = if self.settings.model.is_empty() {
            label("No model configured", "尚未配置模型")
        } else {
            self.settings.model.clone()
        };
        ui.add(Label::new(RichText::new(&model).small().color(palette.fg_muted)).truncate(true))
            .on_hover_text(model);
        if let Some(database) = editor.database_label().or(editor.db_type.as_deref()) {
            let scope = match editor.schema.as_deref() {
                Some(schema) => format!("{database} / {schema}"),
                None => database.to_owned(),
            };
            ui.add(
                Label::new(RichText::new(&scope).small().color(palette.fg_muted)).truncate(true),
            )
            .on_hover_text(label(
                "Table structure is included automatically",
                "提问时自动附带当前数据库的表结构",
            ));
        }
        ui.add_space(6.0);

        ui.add_enabled_ui(self.request.is_none(), |ui| {
            ui.horizontal(|ui| {
                if ui.button(label("+ New chat", "+ 新对话")).clicked() {
                    self.active = None;
                    self.input.clear();
                    self.status.clear();
                }
                let previous = self.active.clone();
                let title = self.active_chat().map_or_else(
                    || label("History", "历史对话"),
                    |chat| {
                        let mut title: String = chat.title.chars().take(10).collect();
                        if chat.title.chars().nth(10).is_some() {
                            title.push('…');
                        }
                        title
                    },
                );
                ComboBox::from_id_source("ai_conversations")
                    .selected_text(title)
                    .width((ui.available_width() - ui.spacing().button_padding.x * 2.0).max(0.0))
                    .show_ui(ui, |ui| {
                        for chat in &self.conversations {
                            ui.selectable_value(
                                &mut self.active,
                                Some(chat.id.clone()),
                                &chat.title,
                            );
                        }
                    });
                if self.active != previous {
                    self.input.clear();
                    self.status.clear();
                }
            });
        });
    }

    fn transcript(
        &self,
        ui: &mut Ui,
        editor: &AiEditorContext,
        palette: Palette,
    ) -> Option<String> {
        let mut suggestion = None;
        ScrollArea::vertical()
            .id_source((&self.active, "ai_chat"))
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                if let Some(chat) = self.active_chat() {
                    for message in &chat.messages {
                        if message.role == "assistant" {
                            ui.label(RichText::new("AI").small().strong().color(palette.info));
                            ui.add(Label::new(&message.content).wrap(true).selectable(true));
                            if self.request.is_none() {
                                for sql in prompts::extract_sql(&message.content) {
                                    if ui
                                        .button(if editor.connection_id.is_some() {
                                            label("Open in new query tab", "在新查询标签页打开")
                                        } else {
                                            label("Copy SQL", "复制 SQL")
                                        })
                                        .clicked()
                                    {
                                        suggestion = Some(sql);
                                    }
                                }
                            }
                        } else {
                            Frame::none()
                                .fill(palette.surface)
                                .stroke(Stroke::new(1.0, palette.border))
                                .rounding(8.0)
                                .inner_margin(Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.label(RichText::new(label("You", "你")).small().strong());
                                    ui.add(
                                        Label::new(&message.content).wrap(true).selectable(true),
                                    );
                                });
                        }
                        ui.add_space(14.0);
                    }
                } else {
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new(label("Ask about your SQL", "有什么 SQL 问题？"))
                            .size(18.0)
                            .strong(),
                    );
                    ui.weak(label(
                        "Generate, explain, or optimize a query.",
                        "可以生成、解释或优化查询。",
                    ));
                }
                if let Some(request) = &self.request {
                    ui.label(RichText::new("AI").small().strong().color(palette.info));
                    if !request.reasoning.is_empty() {
                        ui.collapsing(label("Reasoning", "推理过程"), |ui| {
                            ui.add(Label::new(&request.reasoning).wrap(true).selectable(true));
                        });
                    }
                    ui.add(Label::new(&request.answer).wrap(true).selectable(true));
                }
            });
        suggestion
    }

    fn composer(
        &mut self,
        ui: &mut Ui,
        runtime: &Handle,
        editor: &AiEditorContext,
        palette: Palette,
    ) {
        let busy = self.request.is_some();
        if !self.status.is_empty() {
            ui.add(Label::new(RichText::new(&self.status).small()).truncate(true))
                .on_hover_text(&self.status);
        }
        ui.horizontal(|ui| {
            ui.add_enabled(
                !busy && editor.current_sql.is_some(),
                egui::Checkbox::new(&mut self.include_sql, label("Current SQL", "当前 SQL")),
            )
            .on_hover_text(label(
                "Send the current SQL or selection. Fix SQL also includes the last query error.",
                "发送当前 SQL 或选区。修复 SQL 时还会发送最近一次查询报错。",
            ));
        });
        let input = ScrollArea::vertical()
            .id_source("ai_prompt")
            .max_height(PROMPT_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled(
                    !busy,
                    TextEdit::multiline(&mut self.input)
                        .desired_width(f32::INFINITY)
                        .desired_rows(4)
                        .char_limit(MAX_CONTEXT_CHARS)
                        .hint_text(label("Ask about SQL…", "输入 SQL 问题…")),
                )
            })
            .inner;
        let shortcut = !busy
            && input.has_focus()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        let mut send = false;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), ui.spacing().interact_size.y.max(26.0)),
            Layout::right_to_left(Align::Center),
            |ui| {
                if let Some(request) = &self.request {
                    if ui.button(label("Stop", "停止")).clicked() {
                        request.cancel.notify_one();
                    }
                    ui.spinner();
                } else {
                    send = ui
                        .add_enabled(
                            self.settings_job.is_none() && !self.input.trim().is_empty(),
                            Button::new(
                                RichText::new(label("Send ↑", "发送 ↑"))
                                    .strong()
                                    .color(palette.info),
                            )
                            .fill(palette.accent)
                            .min_size(egui::vec2(76.0, 26.0)),
                        )
                        .on_hover_text(label("Send (Ctrl/⌘+Enter)", "发送（Ctrl/⌘+Enter）"))
                        .clicked();
                }

                ui.add_enabled_ui(!busy, |ui| {
                    ComboBox::from_id_source("ai_action")
                        .selected_text(action_label(self.action))
                        .width(104.0)
                        .show_ui(ui, |ui| {
                            for action in [
                                AiAction::General,
                                AiAction::Generate,
                                AiAction::Explain,
                                AiAction::Optimize,
                                AiAction::Fix,
                                AiAction::Convert,
                                AiAction::SampleData,
                            ] {
                                ui.selectable_value(&mut self.action, action, action_label(action));
                            }
                        });
                });
            },
        );
        if send || (shortcut && self.settings_job.is_none()) {
            if let Err(error) = self.send(ui.ctx(), runtime, editor.clone()) {
                self.status = error;
            }
        }
    }

    fn active_chat(&self) -> Option<&AiConversation> {
        self.conversations
            .iter()
            .find(|chat| Some(&chat.id) == self.active.as_ref())
    }

    fn send(
        &mut self,
        ctx: &Context,
        runtime: &Handle,
        mut editor: AiEditorContext,
    ) -> Result<(), String> {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return Ok(());
        }
        if self.settings.model.is_empty() {
            self.settings_open = true;
            return Ok(());
        }
        let settings = self.settings.clone();
        if !self.include_sql {
            editor.current_sql = None;
        }
        let zh = i18n::locale() == Locale::ZhCn;
        if !self.include_sql || self.action != AiAction::Fix {
            editor.last_error = None;
        }
        let mut context = AiContext {
            editor,
            schema_truncated: false,
            tables: Vec::new(),
            table_names: Vec::new(),
        };
        let action = self.action;
        let user_prompt = prompts::build_user_prompt(action, &text, zh);
        let mut remaining = MAX_CONTEXT_CHARS.saturating_sub(user_prompt.chars().count());
        let mut messages = Vec::new();
        if let Some(chat) = self.active_chat() {
            for message in chat.messages.iter().rev() {
                let size = message.content.chars().take(remaining + 1).count();
                if size > remaining {
                    break;
                }
                remaining -= size;
                messages.push(AiMessage {
                    role: message.role.clone(),
                    content: message.content.clone(),
                });
            }
            messages.reverse();
            if messages
                .first()
                .is_some_and(|message| message.role == "assistant")
            {
                messages.remove(0);
            }
        }
        messages.push(AiMessage {
            role: "user".into(),
            content: user_prompt,
        });
        let chat_id = self.active.clone().unwrap_or_else(|| {
            let chat = AiConversation::new(&text);
            let id = chat.id.clone();
            self.conversations.insert(0, chat);
            id
        });
        self.active = Some(chat_id);
        self.append("user", text.clone());
        self.input.clear();
        self.include_sql = false;
        self.status.clear();
        let (sender, events) = mpsc::unbounded_channel();
        let cancel = Arc::new(Notify::new());
        let request_cancel = cancel.clone();
        let repaint = ctx.clone();
        let data_dir = self.data_dir.clone();
        let metadata = self.metadata.clone();
        runtime.spawn(async move {
            let result = async {
                let config = load_config(&data_dir, &settings, String::new()).await?;
                tokio::select! {
                    _ = request_cancel.notified() => {
                        return Err(client::AGENT_CANCELLED_ERROR.to_owned());
                    }
                    _ = super::schema::load(&metadata, &mut context, &text) => {}
                }
                let system = prompts::build_system_prompt(
                    action,
                    &context,
                    &settings.custom_instructions,
                    zh,
                );
                client::stream_chat(
                    &config,
                    &system,
                    &messages,
                    4096,
                    &request_cancel,
                    &|text, reasoning| {
                        let _ = sender.send(Event::Delta(text.to_owned(), reasoning.to_owned()));
                        repaint.request_repaint_after(Duration::from_millis(30));
                    },
                )
                .await
                .map_err(|error| redact_error(error, &config.api_key))
            }
            .await;
            let _ = sender.send(Event::Finished(result));
            repaint.request_repaint();
        });
        self.request = Some(Request {
            events,
            cancel,
            answer: String::new(),
            reasoning: String::new(),
        });
        Ok(())
    }

    fn poll(&mut self) {
        let Some(request) = &mut self.request else {
            return;
        };
        let result = loop {
            match request.events.try_recv() {
                Ok(Event::Delta(text, reasoning)) => {
                    request.answer.push_str(&text);
                    request.reasoning.push_str(&reasoning);
                }
                Ok(Event::Finished(result)) => break result,
                Err(mpsc::error::TryRecvError::Empty) => return,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    break Err(label("AI request ended unexpectedly.", "AI 请求意外结束。"))
                }
            }
        };
        let answer = std::mem::take(&mut request.answer);
        self.request = None;
        self.status = match result {
            Ok(()) if answer.trim().is_empty() => {
                label("The model returned no answer.", "模型没有返回回答。")
            }
            Ok(()) => String::new(),
            Err(error) if client::is_cancelled_error(&error) => label("Stopped.", "已停止。"),
            Err(error) => error,
        };
        if !answer.trim().is_empty() {
            self.append("assistant", answer);
        }
        self.conversations
            .sort_by_key(|chat| std::cmp::Reverse(chat.updated_at));
        self.conversations.truncate(MAX_CONVERSATIONS);
        if let Err(error) = conversation::save_conversations(&self.data_dir, &self.conversations) {
            self.status = format!("{} {error}", self.status);
        }
    }

    fn append(&mut self, role: &str, content: String) {
        if let Some(chat) = self
            .conversations
            .iter_mut()
            .find(|chat| Some(&chat.id) == self.active.as_ref())
        {
            let now = conversation::now_ms();
            chat.updated_at = now;
            chat.messages.push(AiChatMessage {
                role: role.into(),
                content,
                created_at: now,
                action: None,
            });
        }
    }
}

async fn persist_settings(
    data_dir: &std::path::Path,
    mut settings: AiSettings,
    previous: Option<db_common::CredentialRef>,
    new_key: &str,
) -> Result<AiSettings, String> {
    client::validate_endpoint_and_model(&config_for(&settings, String::new())?)?;
    let store = secret_store(data_dir)?;
    let created = if new_key.is_empty() {
        None
    } else {
        Some(
            store
                .put_secret(API_KEY_LABEL, new_key)
                .await
                .map_err(|error| error.to_string())?,
        )
    };
    if let Some(reference) = &created {
        settings.credential_ref = Some(reference.clone());
    }
    if let Err(error) = settings.save(data_dir) {
        if let Some(reference) = created {
            let _ = store.delete_secret(&reference).await;
        }
        return Err(error);
    }
    if let Some(reference) =
        previous.filter(|reference| Some(reference) != settings.credential_ref.as_ref())
    {
        let _ = store.delete_secret(&reference).await;
    }
    Ok(settings)
}

fn redact_error(error: String, key: &str) -> String {
    if key.is_empty() {
        error
    } else {
        error.replace(key, "[redacted]")
    }
}

async fn load_config(
    data_dir: &std::path::Path,
    settings: &AiSettings,
    mut key: String,
) -> Result<AiConfig, String> {
    if key.is_empty() {
        if let Some(reference) = &settings.credential_ref {
            key = secret_store(data_dir)?
                .get_secret(reference)
                .await
                .map_err(|error| error.to_string())?
                .unwrap_or_default();
        }
    }
    config_for(settings, key)
}

fn config_for(settings: &AiSettings, api_key: String) -> Result<AiConfig, String> {
    let provider: AiProvider = from_setting(&settings.provider)
        .ok_or_else(|| label("Invalid provider.", "无效的服务商。"))?;
    let preset = client::preset_for(provider);
    Ok(AiConfig {
        provider,
        api_key,
        endpoint: settings.endpoint.clone(),
        model: settings.model.clone(),
        models: settings.models.clone(),
        api_style: from_setting::<AiApiStyle>(&settings.api_style)
            .or(preset.map(|item| item.default_api_style))
            .unwrap_or_default(),
        auth_method: from_setting(&settings.auth_method)
            .or(preset.map(|item| item.default_auth_method))
            .unwrap_or_default(),
        proxy_enabled: settings.proxy_enabled,
        proxy_url: settings.proxy_url.clone(),
        enable_thinking: settings.enable_thinking,
        context_window: settings.context_window,
        max_retries: client::clamp_max_retries(settings.max_retries),
    })
}

fn action_label(action: AiAction) -> String {
    match action {
        AiAction::General => label("Ask", "问答"),
        AiAction::Generate => label("Generate SQL", "生成 SQL"),
        AiAction::Explain => label("Explain SQL", "解释 SQL"),
        AiAction::Optimize => label("Optimize SQL", "优化 SQL"),
        AiAction::Fix => label("Fix SQL", "修复 SQL"),
        AiAction::Convert => label("Convert dialect", "转换方言"),
        AiAction::SampleData => label("Generate sample data", "生成示例数据"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("orvyn-ai-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            if let Ok(path) = self.0.canonicalize() {
                if std::env::temp_dir().canonicalize().ok().as_deref() == path.parent()
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("orvyn-ai-"))
                {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
    }

    #[tokio::test]
    async fn settings_round_trip_keeps_key_out_of_plaintext_settings() {
        let temp = TestDirectory::new();
        let key = "test-only-credential";
        let settings = AiSettings::default_for_provider("openai");
        let saved = persist_settings(&temp.0, settings, None, key)
            .await
            .unwrap();
        let config = load_config(&temp.0, &AiSettings::load(&temp.0), String::new())
            .await
            .unwrap();
        assert_eq!(config.api_key, key);
        for name in [super::super::settings::SETTINGS_FILE, "ai-secrets.json"] {
            assert!(!std::fs::read_to_string(temp.0.join(name))
                .unwrap()
                .contains(key));
        }
        let previous = saved.credential_ref.clone();
        let cleared = AiSettings {
            credential_ref: None,
            ..saved
        };
        persist_settings(&temp.0, cleared, previous.clone(), "")
            .await
            .unwrap();
        assert!(secret_store(&temp.0)
            .unwrap()
            .get_secret(&previous.unwrap())
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn completed_conversation_reopens_with_selectable_sql() {
        let temp = TestDirectory::new();
        let metadata =
            MetadataService::with_default_readers(app_core::connection::ConnectionManager::new(
                Arc::new(app_core::storage::MemoryConnectionProfileStore::default()),
                Arc::new(secret_store(&temp.0).unwrap()),
                Vec::new(),
            ));
        let mut panel = AiPanel::new(temp.0.clone(), metadata.clone());
        let chat = AiConversation::new("生成示例 SQL");
        panel.active = Some(chat.id.clone());
        panel.conversations.push(chat);
        panel.append("user", "生成示例 SQL".to_owned());
        let (sender, events) = mpsc::unbounded_channel();
        sender
            .send(Event::Delta("```sql\nSELECT 1;\n```".into(), String::new()))
            .unwrap();
        sender.send(Event::Finished(Ok(()))).unwrap();
        panel.request = Some(Request {
            events,
            cancel: Arc::new(Notify::new()),
            answer: String::new(),
            reasoning: String::new(),
        });
        panel.poll();
        let reopened = AiPanel::new(temp.0.clone(), metadata);
        let messages = &reopened.active_chat().unwrap().messages;
        assert_eq!(messages.len(), 2);
        assert_eq!(prompts::extract_sql(&messages[1].content), ["SELECT 1;"]);
    }

    #[tokio::test]
    async fn schema_uses_cached_columns_and_loads_missing_table_metadata() {
        use app_core::{connection::ConnectionManager, storage::MemoryConnectionProfileStore};
        use db_common::{
            ConnectionProfile, DataSourceKind, PasswordUpdate, RelationalQueryExecutor,
        };

        let temp = TestDirectory::new();
        let path = temp.0.join("askme.sqlite");
        std::fs::File::create(&path).unwrap();
        let connections = ConnectionManager::new(
            Arc::new(MemoryConnectionProfileStore::default()),
            Arc::new(secret_store(&temp.0).unwrap()),
            Vec::new(),
        );
        let profile = ConnectionProfile::builder(DataSourceKind::Sqlite, "askme")
            .file_path(path.to_string_lossy())
            .build()
            .unwrap();
        db_sqlite::SqliteQueryExecutor::new()
            .execute(
                &profile,
                None,
                &["CREATE TABLE comments (id INTEGER, user_id INTEGER)".to_owned()],
                Default::default(),
            )
            .await
            .unwrap();
        let profile = connections
            .create_profile(profile, PasswordUpdate::Keep)
            .await
            .unwrap();
        let metadata = MetadataService::with_default_readers(connections);
        let mut context = AiContext {
            editor: AiEditorContext {
                connection_id: Some(profile.id),
                db_type: Some("sqlite".to_owned()),
                database: Some(path.to_string_lossy().into_owned()),
                schema: Some("main".to_owned()),
                ..Default::default()
            },
            schema_truncated: false,
            tables: Vec::new(),
            table_names: Vec::new(),
        };
        super::super::schema::load(&metadata, &mut context, "查询用户的评论").await;
        let prompt = prompts::build_system_prompt(AiAction::General, &context, "", true);
        assert!(prompt.contains("Database: askme.sqlite"), "{prompt}");
        assert!(
            prompt.contains("comments") && prompt.contains("user_id"),
            "{prompt}"
        );

        context.editor.cached_tables = std::mem::take(&mut context.tables);
        context.editor.tables.push(super::super::AiTableTarget {
            name: "comments".to_owned(),
            schema: Some("main".to_owned()),
            is_view: false,
        });
        std::fs::remove_file(path).unwrap();
        super::super::schema::load(&metadata, &mut context, "查询用户的评论").await;
        assert!(context.tables[0]
            .columns
            .iter()
            .any(|column| column.name == "user_id"));
    }
}
