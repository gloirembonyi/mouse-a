import { fsExtractText, fsReadDataUrl, getConversationMessages } from "./tauri";
import type { Attachment } from "./chatTypes";

const MAX_FILE_CHARS = 4000;
const CHAT_IMAGE = /\.(png|jpe?g|webp)$/i;
const MAX_CONVERSATION_MESSAGES = 10;

export interface ResolvedAttachments {
  /// Context text to prepend before the user's own message - the model sees
  /// this, the chat transcript doesn't (that only shows the attachment chips).
  contextText: string;
  /// A single image is all send_chat/agent_run support today - the first
  /// image attachment wins if more than one was added.
  imageDataUrl: string | null;
}

// Turns chips the user attached (image / @file / @conversation) into the
// extra context the model actually needs - reading file contents and past
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
      // An image file from the editor goes to the model as the image itself.
      if (CHAT_IMAGE.test(a.path)) {
        if (imageDataUrl) {
          blocks.push(`[Attached image: ${a.path} - not sent, only one image fits in a message]`);
          continue;
        }
        try {
          imageDataUrl = await fsReadDataUrl(a.path);
          blocks.push(`[Attached image: ${a.path}]`);
        } catch (err) {
          blocks.push(`[Attached image: ${a.path} - could not read it: ${err instanceof Error ? err.message : String(err)}]`);
        }
        continue;
      }
      try {
        // Plain text as-is; Word/PowerPoint/spreadsheets as extracted text.
        const content = await fsExtractText(a.path);
        const truncated = content.length > MAX_FILE_CHARS ? `${content.slice(0, MAX_FILE_CHARS)}\n… (truncated)` : content;
        blocks.push(`[Attached file: ${a.path}]\n\`\`\`\n${truncated}\n\`\`\``);
      } catch (err) {
        blocks.push(`[Attached file: ${a.path} - could not read it: ${err instanceof Error ? err.message : String(err)}]`);
      }
      continue;
    }
    if (a.kind === "snippet") {
      // Keep the END of long output - that's where errors usually are.
      const text = a.text.length > MAX_FILE_CHARS * 2 ? `… (earlier output trimmed)\n${a.text.slice(-MAX_FILE_CHARS * 2)}` : a.text;
      blocks.push(`[${a.title}]\n\`\`\`\n${text}\n\`\`\``);
      continue;
    }
    if (a.kind === "conversation") {
      try {
        const msgs = await getConversationMessages(a.conversationId, MAX_CONVERSATION_MESSAGES);
        const lines = msgs.map((m) => `${m.role}: ${m.content.slice(0, 400)}`);
        blocks.push(`[Attached conversation "${a.title}"]\n${lines.join("\n")}`);
      } catch (err) {
        blocks.push(`[Attached conversation "${a.title}" - could not load it: ${err instanceof Error ? err.message : String(err)}]`);
      }
    }
  }

  return { contextText: blocks.join("\n\n"), imageDataUrl };
}
