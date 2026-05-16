use super::CodeGenerator;
use crate::parser::{
    Channel, ChannelBackend, ChannelEvent, ChannelMessage, ChannelRequest, DefaultValue, Enum,
    Field, FieldType, Message, Method, MethodMessage, ParsedSchema, Protocol, Service, Stream,
    TypeRegistry,
};
use anyhow::Result;
use convert_case::{Case, Casing};

#[derive(Default)]
pub struct TypeScriptGenerator;

impl TypeScriptGenerator {
    pub fn new() -> Self {
        Self
    }
}

impl CodeGenerator for TypeScriptGenerator {
    fn generate(&self, schema: &ParsedSchema, type_registry: &TypeRegistry) -> Result<String> {
        let mut code = String::new();

        // インポート文を追加
        code.push_str(&self.generate_imports());
        code.push('\n');

        // 列挙型を生成
        for enum_def in &schema.enums {
            code.push_str(&self.generate_enum(enum_def));
            code.push_str("\n\n");
        }

        // メッセージをインターフェースとして生成
        for message in &schema.messages {
            code.push_str(&self.generate_message(message, type_registry));
            code.push_str("\n\n");
        }

        // プロトコル固有のコードを生成
        if let Some(protocol) = &schema.protocol {
            code.push_str(&self.generate_protocol(protocol, type_registry));
        }

        Ok(code)
    }
}

impl TypeScriptGenerator {
    fn generate_imports(&self) -> String {
        r#"// Auto-generated TypeScript definitions
// DO NOT EDIT MANUALLY

export type Timestamp = string; // ISO-8601 format
export type UUID = string;
export type LanguageCode = string; // ISO 639-1 format
"#
        .to_string()
    }

    fn generate_protocol(&self, protocol: &Protocol, type_registry: &TypeRegistry) -> String {
        let mut code = String::new();

        // プロトコルのネームスペースコメントを生成
        if let Some(namespace) = &protocol.namespace {
            code.push_str(&format!("// Namespace: {}\n", namespace));
            code.push_str(&format!("// Version: {}\n\n", protocol.version));
        }

        // プロトコルの列挙型を生成
        for enum_def in &protocol.enums {
            code.push_str(&self.generate_enum(enum_def));
            code.push_str("\n\n");
        }

        // プロトコルのメッセージを生成
        for message in &protocol.messages {
            code.push_str(&self.generate_message(message, type_registry));
            code.push_str("\n\n");
        }

        // サービスクライアントを生成
        for service in &protocol.services {
            code.push_str(&self.generate_service(service, type_registry));
            code.push_str("\n\n");
        }

        // v0.11.0: channel block 対応 (= Unified Channel narrative の TS catch up)
        for channel in &protocol.channels {
            code.push_str(&self.generate_channel(channel, type_registry));
            code.push_str("\n\n");
        }

        code
    }

    /// v0.11.0: channel block を TS interface + metadata に変換
    ///
    /// 各 channel に対して下記を生成:
    /// - event 型 interface (= stream / datagram 両 backend で同じ form)
    /// - request 型 + その returns 型 (= stream backend のみ、 datagram は disallow)
    /// - `<ChannelName>Meta` const object — channel name / backend / channel_id / from /
    ///   lifetime / event names / request mappings を **type-narrowing** 用 const
    ///
    /// Phase 1 (= v0.11.0 sprint plan の Step 1) では **type 定義のみ**、 runtime SDK
    /// 連動の "Channel client class" は Phase 2 で TS runtime SDK と一緒に追加する。
    fn generate_channel(&self, channel: &Channel, type_registry: &TypeRegistry) -> String {
        let mut code = String::new();

        let backend = channel.backend();
        let backend_str = match backend {
            ChannelBackend::Stream => "stream",
            ChannelBackend::Datagram => "datagram",
        };

        // Section header (= human-readable channel summary)
        code.push_str(&format!(
            "// ════════════════════════════════════════════════\n\
             // Channel: {name} (backend={backend}{channel_id})\n\
             // ════════════════════════════════════════════════\n\n",
            name = channel.name,
            backend = backend_str,
            channel_id = match channel.channel_id {
                Some(id) => format!(", channel_id={id}"),
                None => String::new(),
            },
        ));

        // Event 型 interface を生成
        let mut event_names: Vec<String> = Vec::new();
        for evt in &channel.events {
            code.push_str(&self.generate_channel_event(evt, type_registry));
            code.push_str("\n\n");
            event_names.push(evt.name.clone());
        }

        // Request / response 型 interface を生成 (= stream channel のみ)
        let mut request_mappings: Vec<(String, String, String)> = Vec::new(); // (request, response_or_void)
        for req in &channel.requests {
            // Request 型
            code.push_str(&self.generate_channel_request(req, type_registry));
            code.push_str("\n\n");

            // returns block の response 型
            let response_name = match &req.returns {
                Some(returns) => {
                    code.push_str(&self.generate_channel_message_interface(returns, type_registry));
                    code.push_str("\n\n");
                    returns.name.clone()
                }
                None => "void".to_string(),
            };
            request_mappings.push((req.name.clone(), req.name.clone(), response_name));
        }

        let pascal = channel.name.to_case(Case::Pascal);

        // Type map interfaces (= name → 生成 interface の link、 SDK の
        // EventType<M> / RequestType<M,N> / ResponseType<M,N> がこれ経由で解決)。
        // meta const は string literal しか持てないため、 別 type で interface を束ねる。
        let event_types_name = format!("{}ChannelEventTypes", pascal);
        code.push_str(&format!(
            "/** Event name → 生成 interface の map for \"{}\" (= type-narrowing 用) */\n",
            channel.name
        ));
        if event_names.is_empty() {
            code.push_str(&format!(
                "export type {} = Record<string, never>;\n\n",
                event_types_name
            ));
        } else {
            code.push_str(&format!("export interface {} {{\n", event_types_name));
            for n in &event_names {
                code.push_str(&format!("  {}: {};\n", n, n));
            }
            code.push_str("}\n\n");
        }

        let request_types_name = format!("{}ChannelRequestTypes", pascal);
        code.push_str(&format!(
            "/** Request name → {{ request, response }} 生成 interface の map for \"{}\" */\n",
            channel.name
        ));
        if request_mappings.is_empty() {
            code.push_str(&format!(
                "export type {} = Record<string, never>;\n\n",
                request_types_name
            ));
        } else {
            code.push_str(&format!("export interface {} {{\n", request_types_name));
            for (req_name, req_type, resp_type) in &request_mappings {
                let resp_ts = if resp_type == "void" { "void" } else { resp_type };
                code.push_str(&format!(
                    "  {}: {{ request: {}; response: {} }};\n",
                    req_name, req_type, resp_ts
                ));
            }
            code.push_str("}\n\n");
        }

        // Channel metadata const (= Phase 2 runtime SDK の type-narrowing 入力)
        let meta_name = format!("{}ChannelMeta", pascal);
        code.push_str(&format!("/** Channel metadata for \"{}\" (= Phase 2 runtime SDK 用 type-narrowing 入力) */\n", channel.name));
        code.push_str(&format!("export const {} = {{\n", meta_name));
        code.push_str(&format!("  name: {:?} as const,\n", channel.name));
        code.push_str(&format!("  backend: {:?} as const,\n", backend_str));
        if let Some(cid) = channel.channel_id {
            code.push_str(&format!("  channelId: {} as const,\n", cid));
        }
        code.push_str(&format!(
            "  from: {:?} as const,\n",
            match channel.from {
                crate::parser::ChannelFrom::Client => "client",
                crate::parser::ChannelFrom::Server => "server",
                crate::parser::ChannelFrom::Either => "either",
            }
        ));
        code.push_str(&format!(
            "  lifetime: {:?} as const,\n",
            match channel.lifetime {
                crate::parser::ChannelLifetime::Transient => "transient",
                crate::parser::ChannelLifetime::Persistent => "persistent",
            }
        ));

        // events 列挙
        if !event_names.is_empty() {
            code.push_str("  events: [");
            for (i, n) in event_names.iter().enumerate() {
                if i > 0 {
                    code.push_str(", ");
                }
                code.push_str(&format!("{:?}", n));
            }
            code.push_str("] as const,\n");
        } else {
            code.push_str("  events: [] as const,\n");
        }

        // requests mapping (= request name → response type name)
        if !request_mappings.is_empty() {
            code.push_str("  requests: {\n");
            for (req_name, _req_type, resp_type) in &request_mappings {
                code.push_str(&format!(
                    "    {}: {{ request: {:?} as const, response: {:?} as const }},\n",
                    req_name, req_name, resp_type
                ));
            }
            code.push_str("  } as const,\n");
        } else {
            code.push_str("  requests: {} as const,\n");
        }

        // Phantom type carrier (= runtime では undefined、 型のみ存在)。
        // SDK の EventType<M> / RequestType<M,N> / ResponseType<M,N> はこの
        // `__types` field 経由で event/request 名 → 生成 interface を解決する。
        code.push_str(&format!(
            "  __types: undefined as unknown as {{ events: {}; requests: {} }},\n",
            event_types_name, request_types_name
        ));

        code.push_str("} as const;\n");

        code
    }

    /// Channel 内 event を TS interface に変換
    fn generate_channel_event(
        &self,
        event: &ChannelEvent,
        type_registry: &TypeRegistry,
    ) -> String {
        let name = &event.name;
        if event.fields.is_empty() {
            format!(
                "/** Event \"{}\" — empty payload */\nexport interface {} {{}}",
                name, name
            )
        } else {
            let fields: Vec<String> = event
                .fields
                .iter()
                .map(|f| self.generate_field(f, type_registry))
                .collect();
            format!(
                "/** Event \"{}\" */\nexport interface {} {{\n{}\n}}",
                name,
                name,
                fields.join("\n")
            )
        }
    }

    /// Channel 内 request を TS interface に変換 (= stream channel のみ呼ばれる)
    fn generate_channel_request(
        &self,
        req: &ChannelRequest,
        type_registry: &TypeRegistry,
    ) -> String {
        let name = &req.name;
        if req.fields.is_empty() {
            format!(
                "/** Request \"{}\" — empty payload */\nexport interface {} {{}}",
                name, name
            )
        } else {
            let fields: Vec<String> = req
                .fields
                .iter()
                .map(|f| self.generate_field(f, type_registry))
                .collect();
            format!(
                "/** Request \"{}\" */\nexport interface {} {{\n{}\n}}",
                name,
                name,
                fields.join("\n")
            )
        }
    }

    /// Channel 内 returns ブロックの response を TS interface に変換
    fn generate_channel_message_interface(
        &self,
        msg: &ChannelMessage,
        type_registry: &TypeRegistry,
    ) -> String {
        let name = &msg.name;
        if msg.fields.is_empty() {
            format!(
                "/** Response \"{}\" — empty payload */\nexport interface {} {{}}",
                name, name
            )
        } else {
            let fields: Vec<String> = msg
                .fields
                .iter()
                .map(|f| self.generate_field(f, type_registry))
                .collect();
            format!(
                "/** Response \"{}\" */\nexport interface {} {{\n{}\n}}",
                name,
                name,
                fields.join("\n")
            )
        }
    }

    fn generate_enum(&self, enum_def: &Enum) -> String {
        let name = &enum_def.name;
        let values: Vec<String> = enum_def
            .values
            .iter()
            .map(|v| format!("  {} = '{}',", v.to_case(Case::Pascal), v))
            .collect();

        format!("export enum {} {{\n{}\n}}", name, values.join("\n"))
    }

    fn generate_message(&self, message: &Message, type_registry: &TypeRegistry) -> String {
        // インラインメッセージはスキップ
        if message.name.starts_with("_inline_") {
            return String::new();
        }

        let name = &message.name;
        let fields: Vec<String> = message
            .fields
            .iter()
            .map(|f| self.generate_field(f, type_registry))
            .collect();

        format!("export interface {} {{\n{}\n}}", name, fields.join("\n"))
    }

    fn generate_field(&self, field: &Field, type_registry: &TypeRegistry) -> String {
        let name = &field.name;
        let ts_type = self.field_type_to_typescript(&field.field_type(), type_registry);

        let optional = if !field.required { "?" } else { "" };

        let mut field_def = format!("  {}{}: {};", name, optional, ts_type);

        // 制約とデフォルト値のJSDocコメントを追加
        let mut comments = Vec::new();

        if let Some(default) = &field.default() {
            comments.push(format!(
                "@default {}",
                self.default_value_to_string(default)
            ));
        }

        if let (Some(min), Some(max)) = (field.constraints().min, field.constraints().max) {
            comments.push(format!("@minimum {} @maximum {}", min, max));
        }

        if let Some(pattern) = &field.constraints().pattern {
            comments.push(format!("@pattern {}", pattern));
        }

        if !comments.is_empty() {
            let comment = format!("  /** {} */\n", comments.join(" "));
            field_def = format!("{}{}", comment, field_def);
        }

        field_def
    }

    #[allow(clippy::only_used_in_recursion)]
    fn field_type_to_typescript(
        &self,
        field_type: &FieldType,
        type_registry: &TypeRegistry,
    ) -> String {
        match field_type {
            FieldType::String => "string".to_string(),
            FieldType::Int | FieldType::Float => "number".to_string(),
            FieldType::Bool => "boolean".to_string(),
            FieldType::Json | FieldType::Object => "any".to_string(),
            FieldType::Array(inner) => {
                format!("{}[]", self.field_type_to_typescript(inner, type_registry))
            }
            FieldType::Map(_, value) => {
                format!(
                    "Record<string, {}>",
                    self.field_type_to_typescript(value, type_registry)
                )
            }
            FieldType::Enum(values) => values
                .iter()
                .map(|v| format!("'{}'", v))
                .collect::<Vec<_>>()
                .join(" | "),
            FieldType::Custom(name) => {
                type_registry.get_typescript_type(name).unwrap_or_else(|| {
                    // snake_caseをTypeScriptの型用にPascalCaseへ変換
                    if name == "timestamp" {
                        "Timestamp".to_string()
                    } else if name == "uuid" {
                        "UUID".to_string()
                    } else if name == "language_code" {
                        "LanguageCode".to_string()
                    } else {
                        name.to_case(Case::Pascal)
                    }
                })
            }
        }
    }

    fn default_value_to_string(&self, default: &DefaultValue) -> String {
        match default {
            DefaultValue::String(s) => format!("'{}'", s),
            DefaultValue::Int(i) => i.to_string(),
            DefaultValue::Float(f) => f.to_string(),
            DefaultValue::Bool(b) => b.to_string(),
            DefaultValue::Null => "null".to_string(),
            DefaultValue::Array(_) => "[]".to_string(),
            DefaultValue::Object(_) => "{}".to_string(),
        }
    }

    fn generate_service(&self, service: &Service, type_registry: &TypeRegistry) -> String {
        let service_name = format!("{}Service", service.name);
        let client_name = format!("{}Client", service.name);

        let mut code = String::new();

        // インラインメッセージのリクエスト/レスポンス型を生成
        code.push_str(&self.generate_inline_types(service, type_registry));

        // サービスインターフェースを生成
        code.push_str(&format!("export interface {} {{\n", service_name));

        for method in &service.methods {
            code.push_str(&self.generate_service_method(method, type_registry));
        }

        for stream in &service.streams {
            code.push_str(&self.generate_service_stream(stream, type_registry));
        }

        code.push_str("}\n\n");

        // クライアントクラスを生成
        code.push_str(&format!("export class {} {{\n", client_name));
        code.push_str("  constructor(private readonly transport: WebSocketTransport) {}\n\n");

        for method in &service.methods {
            code.push_str(&self.generate_client_method(method, type_registry));
        }

        for stream in &service.streams {
            code.push_str(&self.generate_client_stream(stream, type_registry));
        }

        code.push_str("}\n");

        code
    }

    fn generate_inline_types(&self, service: &Service, type_registry: &TypeRegistry) -> String {
        let mut code = String::new();

        for method in &service.methods {
            if let Some(request) = &method.request {
                let type_name = format!("{}Request", method.name.to_case(Case::Pascal));
                code.push_str(&self.generate_inline_message(&type_name, request, type_registry));
                code.push_str("\n\n");
            }

            if let Some(response) = &method.response {
                let type_name = format!("{}Response", method.name.to_case(Case::Pascal));
                code.push_str(&self.generate_inline_message(&type_name, response, type_registry));
                code.push_str("\n\n");
            }
        }

        for stream in &service.streams {
            if let Some(request) = &stream.request {
                let type_name = format!("{}Request", stream.name.to_case(Case::Pascal));
                code.push_str(&self.generate_inline_message(&type_name, request, type_registry));
                code.push_str("\n\n");
            }

            if let Some(response) = &stream.response {
                let type_name = format!("{}Response", stream.name.to_case(Case::Pascal));
                code.push_str(&self.generate_inline_message(&type_name, response, type_registry));
                code.push_str("\n\n");
            }
        }

        code
    }

    fn generate_inline_message(
        &self,
        name: &str,
        message: &MethodMessage,
        type_registry: &TypeRegistry,
    ) -> String {
        let fields: Vec<String> = message
            .fields
            .iter()
            .map(|f| self.generate_field(f, type_registry))
            .collect();

        format!("export interface {} {{\n{}\n}}", name, fields.join("\n"))
    }

    fn generate_service_method(&self, method: &Method, _type_registry: &TypeRegistry) -> String {
        let name = method.name.to_case(Case::Camel);
        let request_type = self.get_method_type_name(&method.request, &method.name, "Request");
        let response_type = self.get_method_type_name(&method.response, &method.name, "Response");

        format!(
            "  {}(request: {}): Promise<{}>;\n",
            name, request_type, response_type
        )
    }

    fn generate_service_stream(&self, stream: &Stream, _type_registry: &TypeRegistry) -> String {
        let name = stream.name.to_case(Case::Camel);
        let request_type = self.get_method_type_name(&stream.request, &stream.name, "Request");
        let response_type = self.get_method_type_name(&stream.response, &stream.name, "Response");

        format!(
            "  {}(request: {}): AsyncIterableIterator<{}>;\n",
            name, request_type, response_type
        )
    }

    fn generate_client_method(&self, method: &Method, _type_registry: &TypeRegistry) -> String {
        let name = method.name.to_case(Case::Camel);
        let request_type = self.get_method_type_name(&method.request, &method.name, "Request");
        let response_type = self.get_method_type_name(&method.response, &method.name, "Response");

        format!(
            r#"  async {}(request: {}): Promise<{}> {{
    return this.transport.request('{}', request);
  }}
"#,
            name, request_type, response_type, method.name
        )
    }

    fn generate_client_stream(&self, stream: &Stream, _type_registry: &TypeRegistry) -> String {
        let name = stream.name.to_case(Case::Camel);
        let request_type = self.get_method_type_name(&stream.request, &stream.name, "Request");
        let response_type = self.get_method_type_name(&stream.response, &stream.name, "Response");

        format!(
            r#"  async *{}(request: {}): AsyncIterableIterator<{}> {{
    yield* this.transport.stream('{}', request);
  }}
"#,
            name, request_type, response_type, stream.name
        )
    }

    fn get_method_type_name(
        &self,
        message: &Option<MethodMessage>,
        method_name: &str,
        suffix: &str,
    ) -> String {
        if message.is_some() {
            // MethodMessage は常にインラインなので、メソッド名ベースの型名を生成
            format!("{}{}", method_name.to_case(Case::Pascal), suffix)
        } else {
            "void".to_string()
        }
    }
}

// WebSocketトランスポートインターフェース（生成されたファイルに含まれる）
impl TypeScriptGenerator {
    pub fn generate_transport_interface() -> String {
        r#"// WebSocket Transport Interface
export interface WebSocketTransport {
  request<TRequest, TResponse>(method: string, request: TRequest): Promise<TResponse>;
  stream<TRequest, TResponse>(method: string, request: TRequest): AsyncIterableIterator<TResponse>;
  connect(url: string): Promise<void>;
  disconnect(): Promise<void>;
  isConnected(): boolean;
}

// Basic WebSocket transport implementation
export class WebSocketTransportImpl implements WebSocketTransport {
  private ws: WebSocket | null = null;
  private requestId = 0;
  private pendingRequests = new Map<number, {
    resolve: (value: any) => void;
    reject: (error: Error) => void;
  }>();
  private streamHandlers = new Map<number, (data: any) => void>();

  async connect(url: string): Promise<void> {
    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(url);

      this.ws.onopen = () => resolve();
      this.ws.onerror = (error) => reject(error);
      this.ws.onmessage = (event) => this.handleMessage(event);
      this.ws.onclose = () => this.handleClose();
    });
  }

  async disconnect(): Promise<void> {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  async request<TRequest, TResponse>(method: string, request: TRequest): Promise<TResponse> {
    if (!this.isConnected()) {
      throw new Error('WebSocket not connected');
    }

    const id = ++this.requestId;
    const message = {
      id,
      method,
      type: 'request',
      payload: request,
    };

    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
      this.ws!.send(JSON.stringify(message));
    });
  }

  async *stream<TRequest, TResponse>(method: string, request: TRequest): AsyncIterableIterator<TResponse> {
    if (!this.isConnected()) {
      throw new Error('WebSocket not connected');
    }

    const id = ++this.requestId;
    const message = {
      id,
      method,
      type: 'stream',
      payload: request,
    };

    const queue: TResponse[] = [];
    let resolve: ((value: IteratorResult<TResponse>) => void) | null = null;
    let done = false;

    this.streamHandlers.set(id, (data: any) => {
      if (data.type === 'stream_end') {
        done = true;
        this.streamHandlers.delete(id);
        if (resolve) {
          resolve({ done: true, value: undefined });
        }
      } else if (data.type === 'stream_data') {
        const response = data.payload as TResponse;
        if (resolve) {
          resolve({ done: false, value: response });
          resolve = null;
        } else {
          queue.push(response);
        }
      } else if (data.type === 'error') {
        done = true;
        this.streamHandlers.delete(id);
        if (resolve) {
          resolve({ done: true, value: undefined });
        }
      }
    });

    this.ws!.send(JSON.stringify(message));

    while (!done) {
      if (queue.length > 0) {
        yield queue.shift()!;
      } else {
        await new Promise<IteratorResult<TResponse>>((r) => {
          resolve = r;
        }).then((result) => {
          if (!result.done) {
            return result.value;
          }
        });
      }
    }
  }

  private handleMessage(event: MessageEvent): void {
    try {
      const data = JSON.parse(event.data);

      if (data.type === 'response') {
        const handler = this.pendingRequests.get(data.id);
        if (handler) {
          this.pendingRequests.delete(data.id);
          if (data.error) {
            handler.reject(new Error(data.error));
          } else {
            handler.resolve(data.payload);
          }
        }
      } else if (data.type === 'stream_data' || data.type === 'stream_end' || data.type === 'error') {
        const handler = this.streamHandlers.get(data.id);
        if (handler) {
          handler(data);
        }
      }
    } catch (error) {
      console.error('Failed to parse WebSocket message:', error);
    }
  }

  private handleClose(): void {
    // Reject all pending requests
    for (const [id, handler] of this.pendingRequests) {
      handler.reject(new Error('WebSocket connection closed'));
    }
    this.pendingRequests.clear();
    this.streamHandlers.clear();
  }
}
"#.to_string()
    }
}
