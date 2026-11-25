//! AI prompt assembly.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiAction {
    #[default]
    General,
    Generate,
    Explain,
    Optimize,
    Fix,
    Convert,
    SampleData,
}

fn action_label(action: AiAction) -> &'static str {
    match action {
        AiAction::General => "general",
        AiAction::Generate => "generate",
        AiAction::Explain => "explain",
        AiAction::Optimize => "optimize",
        AiAction::Fix => "fix",
        AiAction::Convert => "convert",
        AiAction::SampleData => "sampleData",
    }
}

pub struct AiSkill {
    pub user_instruction_en: &'static str,
    pub user_instruction_zh: &'static str,
    pub system_rules_en: &'static [&'static str],
    pub system_rules_zh: &'static [&'static str],
    pub output_contract_en: &'static str,
    pub output_contract_zh: &'static str,
}

static SKILL_GENERAL: AiSkill = AiSkill {
    user_instruction_en: "Answer the user's question directly and naturally. Use your general knowledge and the database schema context when relevant.",
    user_instruction_zh: "直接、自然地回答用户的问题。使用你的通用知识，涉及数据库时可参考 Schema 上下文。",
    system_rules_en: &[
        "Answer naturally and helpfully. Adapt to the user's intent — whether that's a greeting, a conceptual question, or a database-related inquiry.",
    ],
    system_rules_zh: &["自然、有帮助地回答。根据用户意图灵活应对——无论是问候、概念性问题还是数据库相关咨询。"],
    output_contract_en: "Provide a clear, helpful answer adapted to the user's question.",
    output_contract_zh: "根据用户问题提供清晰、有帮助的回答。",
};

static SKILL_GENERATE: AiSkill = AiSkill {
    user_instruction_en: "Generate a SQL query that satisfies the user's request. Return the SQL in a ```sql code block first, followed by a brief note if needed. Use foreign key relationships from the schema to infer correct JOIN conditions.",
    user_instruction_zh: "根据用户请求生成 SQL。先在 ```sql 代码块中返回 SQL，必要时附简短说明。利用 Schema 中的外键关系推断正确的 JOIN 条件。",
    system_rules_en: &["Use foreign key relationships to infer JOIN conditions. Return the SQL first and avoid long explanations."],
    system_rules_zh: &["利用外键关系推断 JOIN 条件。生成操作优先返回 SQL，避免长篇解释。"],
    output_contract_en: "Output format: put only the final recommended SQL in the first ```sql code block; add at most 3 practical notes after it. If required information is missing, ask one clarifying question first.",
    output_contract_zh: "输出格式：第一个 ```sql 代码块只放最终推荐 SQL；SQL 后最多 3 条实用说明。信息不足时先提出一个澄清问题。",
};

static SKILL_EXPLAIN: AiSkill = AiSkill {
    user_instruction_en: "Explain the current SQL step by step. Point out risky operations, implicit assumptions, and potential performance issues. Reference index and foreign key info from the schema when relevant.",
    user_instruction_zh: "逐步解释当前 SQL。指出危险操作、隐含假设和潜在性能问题。结合 Schema 中的索引和外键信息分析。",
    system_rules_en: &["Explain what the SQL does without changing it. Reference schema, indexes, foreign keys, result preview, and risky assumptions when relevant."],
    system_rules_zh: &["解释当前 SQL 的作用，不要改写 SQL。必要时结合 Schema、索引、外键、结果预览和风险假设说明。"],
    output_contract_en: "Output format: summarize the SQL purpose first, then explain execution logic, risks, and performance notes step by step.",
    output_contract_zh: "输出格式：先概括 SQL 目的，再按步骤解释执行逻辑、风险点和性能注意事项。",
};

static SKILL_OPTIMIZE: AiSkill = AiSkill {
    user_instruction_en: "Rewrite or suggest improvements for the current SQL. Return the improved SQL in a ```sql code block first, followed by short notes explaining the changes. Use the index information in the schema to suggest index-aware optimizations (e.g., avoid full table scans, leverage existing indexes).",
    user_instruction_zh: "重写或优化当前 SQL。先在 ```sql 代码块中返回优化后的 SQL，然后简要说明改动。利用 Schema 中的索引信息建议索引友好的优化（如避免全表扫描、利用现有索引）。",
    system_rules_en: &["Use the index information in the schema to suggest optimizations. Point out which conditions hit indexes and which cause full table scans."],
    system_rules_zh: &["利用 Schema 中的索引信息建议优化。指出哪些查询条件可以命中索引、哪些会导致全表扫描。"],
    output_contract_en: "Output format: provide the optimized SQL first, then explain the key changes in at most 3 notes.",
    output_contract_zh: "输出格式：先给优化后的 SQL，再用最多 3 条说明解释关键改动。",
};

static SKILL_FIX: AiSkill = AiSkill {
    user_instruction_en: "Fix the current SQL using the provided error message and result context. Return the corrected SQL in a ```sql code block first, followed by a brief explanation of the root cause.",
    user_instruction_zh: "根据报错信息和结果上下文修复当前 SQL。先在 ```sql 代码块中返回修正后的 SQL，再简要说明根因。",
    system_rules_en: &["Carefully analyze the error message to identify the root cause. Return the corrected SQL first, then briefly explain."],
    system_rules_zh: &["仔细分析错误信息，定位根因。先返回修正后的 SQL，再简要解释。"],
    output_contract_en: "Output format: corrected SQL, error cause, change notes.",
    output_contract_zh: "输出格式：修复后的 SQL、错误原因、改动说明。",
};

static SKILL_CONVERT: AiSkill = AiSkill {
    user_instruction_en: "Convert the current SQL to the target dialect requested by the user. Return the converted SQL in a ```sql code block first. Note any syntax differences or incompatibilities.",
    user_instruction_zh: "将当前 SQL 转换为用户指定的目标方言。先在 ```sql 代码块中返回转换后的 SQL，再说明语法差异。",
    system_rules_en: &["Convert only to the target dialect requested by the user. Preserve the query intent and call out syntax that cannot be converted safely."],
    system_rules_zh: &["只转换到用户指定的目标方言。保持查询意图，并指出无法安全转换的语法。"],
    output_contract_en: "Output format: provide the converted SQL first, then note important target-dialect syntax differences or incompatibilities.",
    output_contract_zh: "输出格式：先给转换后的 SQL，再说明目标方言下的重要语法差异或不兼容点。",
};

static SKILL_SAMPLE_DATA: AiSkill = AiSkill {
    user_instruction_en: "Generate safe sample INSERT statements or mock data for the current schema. Do not use real production data. Return SQL in a ```sql code block.",
    user_instruction_zh: "为当前 Schema 生成安全的示例 INSERT 语句或模拟数据。不使用真实生产数据。在 ```sql 代码块中返回 SQL。",
    system_rules_en: &["Generate mock data only. Do not use or imply real production data, credentials, personal data, or secrets."],
    system_rules_zh: &["只生成模拟数据。不要使用或暗示真实生产数据、凭据、个人数据或密钥。"],
    output_contract_en: "Output format: provide safe sample SQL first, then explain which values are mock data.",
    output_contract_zh: "输出格式：先给安全的示例 SQL，再说明哪些值是模拟数据。",
};

pub fn skill_for(action: AiAction, _zh: bool) -> &'static AiSkill {
    match action {
        AiAction::General => &SKILL_GENERAL,
        AiAction::Generate => &SKILL_GENERATE,
        AiAction::Explain => &SKILL_EXPLAIN,
        AiAction::Optimize => &SKILL_OPTIMIZE,
        AiAction::Fix => &SKILL_FIX,
        AiAction::Convert => &SKILL_CONVERT,
        AiAction::SampleData => &SKILL_SAMPLE_DATA,
    }
}

#[derive(Clone, Debug)]
pub struct AiSchemaColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    pub comment: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AiSchemaIndex {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Clone, Debug)]
pub struct AiSchemaForeignKey {
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AiSchemaTable {
    pub name: String,
    pub table_type: String,
    pub comment: Option<String>,
    pub columns: Vec<AiSchemaColumn>,
    pub indexes: Vec<AiSchemaIndex>,
    pub foreign_keys: Vec<AiSchemaForeignKey>,
}

impl AiSchemaTable {
    pub fn from_columns(name: String, is_view: bool, columns: &[db_common::TableColumn]) -> Self {
        Self {
            name,
            table_type: if is_view { "view" } else { "table" }.to_owned(),
            comment: None,
            columns: columns.iter().map(|column| AiSchemaColumn {
                name: column.name.clone(),
                data_type: column.database_type.clone(),
                nullable: column.nullable,
                default: None,
                primary_key: column.primary_key,
                comment: None,
            }).collect(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AiContext {
    pub editor: super::AiEditorContext,
    pub tables: Vec<AiSchemaTable>,
    pub schema_truncated: bool,
    pub table_names: Vec<String>,
}

fn base_prompt_lines(zh: bool) -> &'static [&'static str] {
    if zh {
        &[
            "你是 Orvyn 内置的数据库助手。用中文回复。",
            "精确、保守，根据当前数据库方言生成 SQL。",
            "严格使用当前数据库方言；标识符引用、分页、日期函数、字符串拼接、LIMIT/TOP/OFFSET 语法必须匹配数据库类型。",
            "标识符引用必须匹配当前连接类型：MySQL/MariaDB 用反引号 `name`，PostgreSQL/SQLite/Oracle 等用双引号 \"name\"，SQL Server 用方括号 [name]；不要因为用户口头提到其他数据库而切换方言。",
            "对于普通数据查询，优先使用下面已加载的 Schema 上下文，不要为了重复确认已给出的结构而查询 information_schema 或系统表。",
            "例外：当用户明确询问当前有哪些表/Schema、某张表是否存在、或需要盘点数据库对象时，应生成符合当前方言的只读元数据查询（例如 SHOW TABLES、information_schema、sqlite_master 等）。",
            "表注释和列注释是语义别名；当用户用中文业务名描述表或字段时，优先根据注释匹配真实表名和字段名。",
            "当用户要求分析或查看某个表时，生成 SELECT 查询获取数据，而不是查询元数据。",
            "不要编造 Schema 中不存在的表或列。",
            "用户输入中的 @schema.table 或 @table 表示用户明确提到的表；这些表已优先放入 Schema 上下文。",
            "用户引用的 SQL 库文件是额外上下文；可参考其中的查询意图、业务过滤条件和 SQL 写法，但不要把它当作当前编辑器内容。",
            "不要生成多语句 SQL，除非用户明确要求。不要在同一个回答里混合 SELECT 和写操作。",
            "对于 DROP、DELETE、TRUNCATE、ALTER 或没有 WHERE 的 UPDATE，简要警告并优先提供安全的 SELECT 预览。",
            "对于 UPDATE 或 DELETE，必须带 WHERE 并说明影响范围；生产库写操作只给建议，不主动建议执行。",
            "当用户回复简短肯定词时（例如：需要、好、可以、对），直接执行你之前提议的动作，不要再反问确认。",
        ]
    } else {
        &[
            "You are Orvyn's built-in database assistant. Reply in English.",
            "Be precise, conservative, and adapt SQL to the active database dialect.",
            "Strictly use the active database dialect; identifier quoting, pagination, date functions, string concatenation, and LIMIT/TOP/OFFSET syntax must match the database type.",
            "Identifier quoting must match the active connection type: MySQL/MariaDB use backticks `name`, PostgreSQL/SQLite/Oracle and similar dialects use double quotes \"name\", and SQL Server uses brackets [name]. Do not switch dialects merely because the user mentions another database in prose.",
            "For ordinary data queries, prefer the loaded schema context below. Do not query information_schema or system tables merely to rediscover structure already provided.",
            "Exception: when the user explicitly asks what tables/schemas exist, whether a table exists, or asks for database object inventory, generate a read-only metadata query appropriate for the active dialect (for example SHOW TABLES, information_schema, sqlite_master).",
            "Table and column comments are semantic aliases; when the user describes tables or fields by business names, prefer matching those comments to the real table and column names.",
            "When the user asks to 'analyze' or 'look at' a table, generate a SELECT query to retrieve data, not a metadata query.",
            "Never invent tables or columns that are not in the schema context.",
            "User input may contain @schema.table or @table mentions. Treat them as explicit table references; mentioned tables are prioritized in the schema context.",
            "Referenced SQL library files are additional context. Use them for query intent, business filters, and SQL style, but do not treat them as the current editor content.",
            "Do not generate multi-statement SQL unless the user explicitly asks for it. Do not mix SELECT statements and write operations in the same answer.",
            "For destructive statements (DROP, DELETE, TRUNCATE, ALTER, UPDATE without WHERE), warn briefly and prefer a safer SELECT preview.",
            "For UPDATE or DELETE, require a WHERE clause and explain the affected scope; for production writes, provide guidance but do not proactively suggest execution.",
            "When the user replies with a short affirmative (e.g.: Yes, OK, Sure, Do it), directly execute the action you previously proposed — do not ask for confirmation again.",
        ]
    }
}

fn ask_mode_line(zh: bool, db_type: &str) -> &'static str {
    if db_type.eq_ignore_ascii_case("mongodb") {
        if zh {
            "你处于 MongoDB Ask 模式。只生成 MongoDB shell 风格命令和说明，不生成 SQL，也不要暗示已经执行或即将自动执行。"
        } else {
            "You are in MongoDB Ask mode. Generate MongoDB shell-style commands and explanations, not SQL, and do not imply that anything has run or will auto-run."
        }
    } else if zh {
        "你处于 Ask 模式。只生成 SQL 和说明，不要暗示已经执行或即将自动执行。"
    } else {
        "You are in Ask mode. Generate SQL and explanations only; do not imply that anything has run or will auto-run."
    }
}

pub fn build_system_prompt(
    action: AiAction,
    ctx: &AiContext,
    custom_instructions: &str,
    zh: bool,
) -> String {
    let skill = skill_for(action, zh);
    let editor = &ctx.editor;

    let mut lines: Vec<String> = base_prompt_lines(zh)
        .iter()
        .map(|line| line.to_string())
        .collect();
    lines.push(ask_mode_line(zh, editor.db_type.as_deref().unwrap_or_default()).to_string());
    if zh {
        for rule in skill.system_rules_zh {
            lines.push((*rule).to_string());
        }
        lines.push(skill.output_contract_zh.to_string());
    } else {
        for rule in skill.system_rules_en {
            lines.push((*rule).to_string());
        }
        lines.push(skill.output_contract_en.to_string());
    }

    let custom = custom_instructions.trim();
    if !custom.is_empty() {
        if zh {
            lines.push(format!(
                "## 用户自定义规范（补充性）\n以下为用户定义的规范与模板；上方核心安全及方言规则优先级更高。\n\n{custom}"
            ));
        } else {
            lines.push(format!(
                "## Custom Instructions (supplementary)\nThe following are user-defined conventions and templates. Core safety and dialect rules above take precedence.\n\n{custom}"
            ));
        }
    }

    if ctx.schema_truncated {
        if zh {
            lines.push(
                "仅提供了部分表结构。对缺少字段定义的表，请生成只读的元数据查询，或请用户指定相关表。".to_string(),
            );
        } else {
            lines.push(
                "Only partial schema is available. For tables without column definitions, generate a read-only metadata query or ask the user to specify the relevant table."
                    .to_string(),
            );
        }
    }

    if zh {
        lines.push("返回 SQL 时放在 ```sql 代码块中。额外说明简短实用。".to_string());
    } else {
        lines.push(
            "Put SQL in a fenced ```sql code block. Keep extra explanation short and practical."
                .to_string(),
        );
    }

    if let Some(db_type) = editor
        .db_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Database type: {db_type}"));
    }
    if let Some(connection_name) = editor
        .connection_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Connection: {connection_name}"));
    }
    if let Some(database) = editor
        .database_label()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Database: {database}"));
    }
    if let Some(schema) = editor
        .schema
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Selected schema: {schema}"));
    }
    if let Some(current_sql) = editor.current_sql.as_deref() {
        let trimmed = current_sql.trim();
        lines.push(format!(
            "Current SQL:\n{}",
            if trimmed.is_empty() {
                "(empty)"
            } else {
                trimmed
            }
        ));
    }
    if let Some(last_error) = editor
        .last_error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("\nLast error:\n{last_error}\n"));
    }
    if let Some(preview) = editor
        .last_result_preview
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("\nLast result preview:\n{preview}\n"));
    }
    if !ctx.table_names.is_empty() {
        lines.push(format!("Known table names: {}", ctx.table_names.join(", ")));
    }
    lines.push(format!("Schema:\n{}", format_schema(&ctx.tables)));

    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}

pub fn build_user_prompt(action: AiAction, instruction: &str, zh: bool) -> String {
    let skill = skill_for(action, zh);
    let user_request = instruction.trim();
    let fallback = if zh {
        "（无额外说明）"
    } else {
        "(No extra instruction provided.)"
    };
    let skill_instruction = if zh {
        skill.user_instruction_zh
    } else {
        skill.user_instruction_en
    };
    format!(
        "Action: {}\n{}\n\nUser request:\n{}",
        action_label(action),
        skill_instruction,
        if user_request.is_empty() {
            fallback
        } else {
            user_request
        }
    )
}

fn format_schema(tables: &[AiSchemaTable]) -> String {
    if tables.is_empty() {
        return "(No table schema loaded.)".to_string();
    }
    tables
        .iter()
        .map(|table| {
            let mut lines: Vec<String> = vec![format!("{} ({})", table.name, table.table_type)];
            if let Some(comment) = table
                .comment
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                lines.push(format!("  Comment: {comment}"));
            }

            for column in &table.columns {
                let mut flags: Vec<String> = Vec::new();
                if column.primary_key {
                    flags.push("PK".to_string());
                }
                flags.push(if column.nullable {
                    "nullable".to_string()
                } else {
                    "NOT NULL".to_string()
                });
                if let Some(default) = column
                    .default
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    flags.push(format!("default {default}"));
                }
                let mut line = format!(
                    "  - {}: {} ({})",
                    column.name,
                    column.data_type,
                    flags.join(", ")
                );
                if let Some(comment) = column
                    .comment
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    line.push_str(&format!(" -- {comment}"));
                }
                lines.push(line);
            }

            for index in &table.indexes {
                let unique = if index.unique { "UNIQUE " } else { "" };
                lines.push(format!(
                    "  Index: {unique}{}({})",
                    index.name,
                    index.columns.join(", ")
                ));
            }

            for fk in &table.foreign_keys {
                lines.push(format!(
                    "  FK: {} → {}.{}",
                    fk.columns.join(", "),
                    fk.ref_table,
                    fk.ref_columns.join(", ")
                ));
            }

            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn is_sql_fence_language(info: &str) -> bool {
    let language = info
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        language.as_str(),
        "sql"
            | "mysql"
            | "postgresql"
            | "postgres"
            | "sqlite"
            | "tsql"
            | "sqlserver"
            | "transact-sql"
            | "clickhouse"
    )
}

pub fn extract_sql(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut opening = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        let marker_len = trimmed.bytes().take_while(|byte| *byte == b'`').count();
        if let Some((start, required, is_sql)) = opening {
            if marker_len >= required && trimmed[marker_len..].trim().is_empty() {
                let sql = text[start..offset].trim();
                if is_sql && !sql.is_empty() {
                    blocks.push(sql.to_owned());
                }
                opening = None;
            }
        } else if marker_len >= 3 && !trimmed[marker_len..].contains('`') {
            opening = Some((
                offset + line.len(),
                marker_len,
                is_sql_fence_language(&trimmed[marker_len..]),
            ));
        }
        offset += line.len();
    }
    blocks
}
