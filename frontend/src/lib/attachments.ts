import { fsReadTextFile, getConversationMessages } from "./tauri";
import type { Attachment } from "./chatTypes";

const MAX_FILE_CHARS = 4000;
const MAX_CONVERSATION_MESSAGES = 10;

export interface ResolvedAttachments {
  /// Context text to prepend before the user's own message — the model sees
  /// this, the chat transcript doesn't (that only shows the attachment chips).
  contextText: string;
  /// A single image is all send_chat/agent_run support today — the first
  /// image attachment wins if more than one was added.
  imageDataUrl: string | null;
}

// Turns chips the user attached (image / @file / @conversation) into the
// extra context the model actually needs — reading file contents and past
// conversation turns live, right before sending, so the prompt reflects
// their CURRENT state rather than whatever they were when tagged.
export async function resolveAttachments(attachments: Attachment[]): Promise<ResolvedAttachments> {
  const blocks: string[] = [];
  let imageDataUrl: string | null = null;

  for (const a of attachments) {
    if (a.kind === "image") {
      if (!imageDataUrl) imageDataUrl = a.dataUrl;
      continue;
    }
    if (a.kind === "file") {
      try {
        const content = await fsReadTextFile(a.path);
        const truncated = content.length > MAX_FILE_CHARS ? `${content.slice(0, MAX_FILE_CHARS)}\n… (truncated)` : content;
        blocks.push(`[Attached file: ${a.path}]\n\`\`\`\n${truncated}\n\`\`\``);
      } catch (err) {
        blocks.push(`[Attached file: ${a.path} — could not read it: ${err instanceof Error ? err.message : String(err)}]`);
      }
      continue;
    }
    if (a.kind === "conversation") {
      try {
        const msgs = await getConversationMessages(a.conversationId, MAX_CONVERSATION_MESSAGES);
        const lines = msgs.map((m) => `${m.role}: ${m.content.slice(0, 400)}`);
        blocks.push(`[Attached conversation "${a.title}"]\n${lines.join("\n")}`);
      } catch (err) {
        blocks.push(`[Attached conversation "${a.title}" — could not load it: ${err instanceof Error ? err.message : String(err)}]`);
      }
    }
  }

  return { contextText: blocks.join("\n\n"), imageDataUrl };
}
