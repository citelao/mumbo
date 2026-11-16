import type { Editor, Extensions } from "@tiptap/core";
import { DOMSerializer } from "@tiptap/pm/model";
import { Document } from "@tiptap/extension-document";
import { Paragraph } from "@tiptap/extension-paragraph";
import { HardBreak } from "@tiptap/extension-hard-break";
import { Text } from "@tiptap/extension-text";
import { Placeholder, UndoRedo } from "@tiptap/extensions";
import { Bold } from "@tiptap/extension-bold";
import { Italic } from "@tiptap/extension-italic";
import { Code } from "@tiptap/extension-code";
import { Underline } from "@tiptap/extension-underline";
import { Link } from "@tiptap/extension-link";
import { FileHandler } from "@tiptap/extension-file-handler";

export interface ChatExtensionOptions {
    onSend: () => void;
    /** Sends a pasted or dropped image, as a data URL, as a message of its own. */
    onSendImage: (dataUrl: string) => void;
}

export function createChatExtensions(options: ChatExtensionOptions): Extensions {
    return [
        // Exactly one paragraph: Link's autolink/paste handling and Placeholder
        // only work on block nodes, so a bare `inline*` doc silently disables
        // them. The paragraph is stripped again in getMessageHtml.
        Document.extend({
            content: "paragraph",
        }),
        Paragraph,
        HardBreak.extend({
            addKeyboardShortcuts() {
                // https://stackoverflow.com/questions/71439281/change-enter-to-break-line-instead-of-new-paragraph-in-tiptap
                return {
                    "Mod-Enter": () => {
                        options.onSend();
                        return true;
                    },
                    "Shift-Enter": () => this.editor.commands.setHardBreak(),
                };
            },
        }),
        Text,

        Bold,
        Code,
        Italic,
        Link.configure({
            // Clicks in the webview would navigate the whole app away.
            openOnClick: false,
            autolink: true,
            linkOnPaste: true,
            defaultProtocol: "https",
        }),
        Underline,

        // No Image extension: images are sent on their own as soon as they're
        // pasted or dropped, and left out of pasted HTML.
        FileHandler.configure({
            allowedMimeTypes: ["image/png", "image/jpeg", "image/gif", "image/webp"],
            // Otherwise, when the clipboard also has HTML for the image (e.g.
            // "Copy Image" in a browser), that gets pasted too.
            consumePasteEvent: true,
            onDrop: (_editor, files) => sendImageFile(files, options.onSendImage),
            onPaste: (_editor, files) => sendImageFile(files, options.onSendImage),
        }),

        UndoRedo,

        Placeholder.configure({
            placeholder: "Type a message...",
        }),
    ];
}

/** Reads a single image file as a data URL and sends it. */
function sendImageFile(files: File[], onSendImage: (dataUrl: string) => void) {
    if (files.length !== 1) {
        alert("Images can only be sent one at a time.");
        return;
    }
    const reader = new FileReader();
    reader.onload = () => {
        if (typeof reader.result !== "string") return;
        onSendImage(reader.result);
    };
    reader.readAsDataURL(files[0]);
}

/** Whether there's nothing worth sending: no non-whitespace text. */
export function isMessageEmpty(editor: Editor): boolean {
    return editor.getText().trim().length === 0;
}

/**
 * The message as XHTML to send over Mumble: the contents of the single
 * paragraph, never wrapped in `<p>`.
 *
 * Must be well-formed XML (`<br />`, no `&nbsp;`): Murmur
 * re-parses messages over its text length limit as XML, and rejects them as
 * `TextTooLong` if that fails. See contrib/text-message-tester.
 */
export function getMessageHtml(editor: Editor): string {
    const paragraph = editor.state.doc.firstChild;
    if (paragraph === null) return "";

    // Nodes from a namespace-less XML document, so XMLSerializer doesn't add
    // xmlns="http://www.w3.org/1999/xhtml" to every top-level element.
    const xmlDocument = document.implementation.createDocument(null, null);
    const fragment = DOMSerializer.fromSchema(editor.schema).serializeFragment(paragraph.content, {
        document: xmlDocument,
    });
    const serializer = new XMLSerializer();
    return Array.from(fragment.childNodes)
        .map((node) => serializer.serializeToString(node))
        .join("")
        .trim();
}
