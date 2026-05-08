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
}

export interface ChatMessage {
  timestamp: number;
  sender_id: string;
  sender_name: string;
  body: string | null;
  attachments: AttachmentInfo[];
  is_outgoing: boolean;
}
