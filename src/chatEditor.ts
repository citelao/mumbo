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
import { Image } from "@tiptap/extension-image";
import { FileHandler } from "@tiptap/extension-file-handler";

export interface ChatExtensionOptions {
    onSend: () => void;
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

        Image.configure({
            allowBase64: true,
            inline: true,
        }),
        FileHandler.configure({
            allowedMimeTypes: ["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf"],
            onDrop: (currentEditor, files, pos) => {
                // TODO.
            },
            onPaste: (currentEditor, files, htmlContent) => {
                // We *could* handle HTML (with images) differently, to save
                // re-encoding e.g., but the official Mumble client only accepts
                // base64 images & refuses to load remote resources (it shows a
                // giant black square instead). So skip.
                //
                // if (htmlContent) {
                //     // Allow other handlers.
                //     console.log(htmlContent);
                //     return false;
                // }

                for (const file of files) {
                    const fileReader = new FileReader();
                    fileReader.readAsDataURL(file);
                    fileReader.onload = () => {
                        const base64 = fileReader.result;
                        if (base64) {
                            currentEditor
                                .chain()
                                .insertContentAt(
                                    currentEditor.state.selection.anchor,
                                    {
                                        type: "image",
                                        attrs: {
                                            src: base64,
                                        },
                                    })
                                .focus()
                                .run();
                        }
                    };
                }
            },
        }),

        UndoRedo,

        Placeholder.configure({
            placeholder: "Type a message...",
        }),
    ];
}

/**
 * The message as HTML to send over Mumble: the contents of the single
 * paragraph, never wrapped in `<p>`.
 */
export function getMessageHtml(editor: Editor): string {
    const paragraph = editor.state.doc.firstChild;
    if (paragraph === null) return "";

    const container = document.createElement("div");
    container.appendChild(
        DOMSerializer.fromSchema(editor.schema).serializeFragment(paragraph.content),
    );
    return container.innerHTML.trim();
}
