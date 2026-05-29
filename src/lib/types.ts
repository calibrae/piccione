export type ProvisioningState =
  | { type: "Idle" }
  | { type: "Connecting" }
  | { type: "WaitingForScan"; qr_code_svg: string }
  | { type: "Provisioning" }
  | { type: "Registered"; device_name: string }
  | { type: "Error"; message: string };

export interface AttachmentInfo {
  id: string;
  file_name: string;
  mime_type: string;
  size: number;
  local_path: string | null;
  pointer_data?: number[] | null;
}

export interface Conversation {
  id: string;
  name: string;
  last_message: string | null;
  last_timestamp: number;
  is_group: boolean;
  avatar_path: string | null;
}

export interface QuotedMessage {
  id: number;
  author_id: string;
  author_name: string;
  text: string;
}

export interface LinkPreview {
  url: string;
  title: string;
  description: string;
}

export interface MsgRange {
  start: number;
  length: number;
  style?: string;
  mention_uuid?: string;
}

export interface PollInfo {
  question: string;
  options: string[];
  allow_multiple: boolean;
}

export interface ChatMessage {
  timestamp: number;
  sender_id: string;
  sender_name: string;
  body: string | null;
  attachments: AttachmentInfo[];
  is_outgoing: boolean;
  quote?: QuotedMessage;
  previews?: LinkPreview[];
  body_ranges?: MsgRange[];
  edited?: boolean;
  poll?: PollInfo;
}

export interface SearchHit {
  conversation_id: string;
  conversation_name: string;
  is_group: boolean;
  timestamp: number;
  sender_name: string;
  snippet: string;
}
