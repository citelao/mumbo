// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { Editor } from "@tiptap/core";
import { createChatExtensions, getMessageHtml, isMessageEmpty, type ChatExtensionOptions } from "./chatEditor";

const editors: Editor[] = [];

function makeEditor({ onSend = () => {}, onSendImage = () => {} }: Partial<ChatExtensionOptions> = {}) {
    const editor = new Editor({
        element: document.body.appendChild(document.createElement("div")),
        extensions: createChatExtensions({ onSend, onSendImage }),
        content: "",
    });
    editors.push(editor);
    return editor;
}

afterEach(() => {
    editors.splice(0).forEach((e) => e.destroy());
    document.body.innerHTML = "";
});

// Char-by-char, as real typing does: Link's autolink only fires on the
// transaction that inserts whitespace after a URL.
function type(editor: Editor, text: string) {
    for (const char of text) {
        const { from, to } = editor.state.selection;
        const handled = editor.view.someProp("handleTextInput", (f) =>
            f(editor.view, from, to, char, () => editor.state.tr.insertText(char, from, to)),
        );
        if (!handled) {
            editor.view.dispatch(editor.state.tr.insertText(char, from, to));
        }
    }
}

function paste(editor: Editor, { html, text, files = [] }: { html?: string; text?: string; files?: File[] }) {
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", {
        value: {
            getData: (type: string) =>
                type === "text/html" ? (html ?? "") : type === "text/plain" ? (text ?? "") : "",
            types: [
                ...(files.length ? ["Files"] : []),
                ...(html ? ["text/html"] : []),
                ...(text ? ["text/plain"] : []),
            ],
            files,
        },
    });
    editor.view.dom.dispatchEvent(event);
}

function drop(editor: Editor, files: File[]) {
    // jsdom has no layout, so ProseMirror can't find a drop position (and
    // ignores the drop) unless we stub it: the start of the paragraph.
    editor.view.posAtCoords = () => ({ pos: 1, inside: -1 });
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperties(event, {
        dataTransfer: { value: { types: ["Files"], files, getData: () => "" } },
        clientX: { value: 0 },
        clientY: { value: 0 },
    });
    editor.view.dom.dispatchEvent(event);
}

function pngFile() {
    return new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "image.png", { type: "image/png" });
}

describe("message HTML", () => {
    it("is empty for an empty editor", () => {
        expect(getMessageHtml(makeEditor())).toBe("");
    });

    it("never wraps the message in <p>", () => {
        const editor = makeEditor();
        type(editor, "hello");
        expect(editor.getHTML()).toContain("<p>"); // sanity: the doc does have a paragraph
        expect(getMessageHtml(editor)).toBe("hello");
    });

    it.each([
        ["multiple pasted paragraphs", "<p>one</p><p>two</p><p>three</p>"],
        ["pasted headings and lists", "<h1>title</h1><ul><li>a</li><li>b</li></ul>"],
        ["pasted blockquote and div", "<blockquote>q</blockquote><div>d</div>"],
        ["nested paragraphs in a table", "<table><tr><td><p>cell</p></td></tr></table>"],
    ])("never contains <p> after %s", (_name, html) => {
        const editor = makeEditor();
        paste(editor, { html, text: "ignored" });
        expect(getMessageHtml(editor)).not.toMatch(/<\/?p[\s>]/i);
        expect(editor.state.doc.childCount).toBe(1);
    });

    it("never contains <p> after a hard break", () => {
        const editor = makeEditor();
        type(editor, "a");
        editor.commands.setHardBreak();
        type(editor, "b");
        expect(getMessageHtml(editor)).toMatch(/^a<br ?\/>b$/);
    });

    // Murmur re-parses long messages (e.g. with images) as XML and rejects
    // them as TextTooLong if that fails. See contrib/text-message-tester.
    it.each([
        ["a hard break", '<p>a<br>b</p>'],
        ["a non-breaking space", "<p>a&nbsp;b</p>"],
        ["an ampersand in text and a link", '<p>a &amp; <a href="https://example.com/?a=1&amp;b=2">b</a></p>'],
    ])("is well-formed XML with %s", (_name, content) => {
        const editor = makeEditor();
        editor.commands.setContent(content);
        const html = getMessageHtml(editor);
        const xml = new DOMParser().parseFromString(`<document>${html}</document>`, "application/xml");
        expect(xml.getElementsByTagName("parsererror")).toHaveLength(0);
        expect(html).not.toContain("xmlns");
    });

    it("never contains <p> after pressing Enter-ish commands", () => {
        const editor = makeEditor();
        type(editor, "a");
        editor.commands.splitBlock();
        editor.commands.enter();
        type(editor, "b");
        expect(editor.state.doc.childCount).toBe(1);
        expect(getMessageHtml(editor)).not.toContain("<p");
    });
});

describe("isMessageEmpty", () => {
    it("is true for an empty editor", () => {
        expect(isMessageEmpty(makeEditor())).toBe(true);
    });

    it("is true for only whitespace and hard breaks", () => {
        const editor = makeEditor();
        type(editor, "  ");
        editor.commands.setHardBreak();
        type(editor, " ");
        expect(isMessageEmpty(editor)).toBe(true);
    });

    it("is false for text", () => {
        const editor = makeEditor();
        type(editor, "hi");
        expect(isMessageEmpty(editor)).toBe(false);
    });
});

// Images are sent on their own as soon as they're pasted or dropped, never
// mixed into the text.
describe("images", () => {
    it("sends a pasted image file at once, as a data URL", async () => {
        const onSendImage = vi.fn();
        const editor = makeEditor({ onSendImage });
        type(editor, "hi");
        paste(editor, { files: [pngFile()] });
        await vi.waitFor(() => expect(onSendImage).toHaveBeenCalledOnce());
        expect(onSendImage).toHaveBeenCalledWith(expect.stringMatching(/^data:image\/png;base64,/));
        // The text being typed stays put.
        expect(getMessageHtml(editor)).toBe("hi");
    });

    it("sends only the file when the clipboard also has HTML for it", async () => {
        const onSendImage = vi.fn();
        const editor = makeEditor({ onSendImage });
        paste(editor, { files: [pngFile()], html: '<img src="https://example.com/image.png">' });
        await vi.waitFor(() => expect(onSendImage).toHaveBeenCalledOnce());
        expect(getMessageHtml(editor)).toBe("");
    });

    it("sends a dropped image file at once, as a data URL", async () => {
        const onSendImage = vi.fn();
        const editor = makeEditor({ onSendImage });
        drop(editor, [pngFile()]);
        await vi.waitFor(() => expect(onSendImage).toHaveBeenCalledOnce());
        expect(onSendImage).toHaveBeenCalledWith(expect.stringMatching(/^data:image\/png;base64,/));
        expect(getMessageHtml(editor)).toBe("");
    });

    it("refuses several images at once", async () => {
        const alert = vi.spyOn(window, "alert").mockImplementation(() => {});
        const onSendImage = vi.fn();
        const editor = makeEditor({ onSendImage });
        paste(editor, { files: [pngFile(), pngFile()] });
        await vi.waitFor(() => expect(alert).toHaveBeenCalledOnce());
        // Give a file read the chance to finish and send.
        await new Promise((resolve) => setTimeout(resolve, 20));
        expect(onSendImage).not.toHaveBeenCalled();
        alert.mockRestore();
    });

    it("leaves images out of pasted HTML", () => {
        const editor = makeEditor();
        paste(editor, { html: '<p>a<img src="data:image/png;base64,AAAA">b</p>', text: "ab" });
        expect(getMessageHtml(editor)).toBe("ab");
    });
});

describe("formatting", () => {
    it.each([
        ["toggleBold", "strong"],
        ["toggleItalic", "em"],
        ["toggleUnderline", "u"],
        ["toggleCode", "code"],
    ] as const)("%s wraps typed text in <%s>", (command, tag) => {
        const editor = makeEditor();
        editor.commands[command]();
        type(editor, "hi");
        expect(getMessageHtml(editor)).toBe(`<${tag}>hi</${tag}>`);
    });

    it("applies bold with Mod-b", () => {
        const editor = makeEditor();
        type(editor, "hi");
        editor.commands.selectAll();
        editor.commands.keyboardShortcut("Mod-b");
        expect(getMessageHtml(editor)).toBe("<strong>hi</strong>");
    });

    it("combines marks", () => {
        const editor = makeEditor();
        editor.commands.toggleBold();
        editor.commands.toggleItalic();
        type(editor, "hi");
        expect(getMessageHtml(editor)).toMatch(/^<(strong><em|em><strong)>hi<\//);
    });

    it("undoes typing", () => {
        const editor = makeEditor();
        type(editor, "hi");
        editor.commands.undo();
        expect(getMessageHtml(editor)).toBe("");
    });
});

describe("keyboard", () => {
    it("Mod-Enter sends", () => {
        const onSend = vi.fn();
        const editor = makeEditor({ onSend });
        editor.commands.keyboardShortcut("Mod-Enter");
        expect(onSend).toHaveBeenCalledOnce();
    });

    it("Shift-Enter inserts a hard break instead of sending", () => {
        const onSend = vi.fn();
        const editor = makeEditor({ onSend });
        type(editor, "a");
        editor.commands.keyboardShortcut("Shift-Enter");
        type(editor, "b");
        expect(onSend).not.toHaveBeenCalled();
        expect(getMessageHtml(editor)).toMatch(/^a<br ?\/>b$/);
    });
});

describe("links", () => {
    it("autolinks a URL when typing a space after it", () => {
        const editor = makeEditor();
        type(editor, "see https://example.com now");
        const html = getMessageHtml(editor);
        expect(html).toContain('<a ');
        expect(html).toContain('href="https://example.com"');
        expect(html).toMatch(/>https:\/\/example\.com<\/a> now$/);
    });

    it("does not link ordinary words", () => {
        const editor = makeEditor();
        type(editor, "hello world ");
        expect(getMessageHtml(editor)).not.toContain("<a ");
    });

    it("links a bare URL pasted as plain text", () => {
        const editor = makeEditor();
        paste(editor, { text: "https://example.com/a?b=1" });
        const html = getMessageHtml(editor);
        expect(html).toContain('href="https://example.com/a?b=1"');
        expect(html).not.toContain("<p");
    });

    it("links a URL pasted over selected text", () => {
        const editor = makeEditor();
        type(editor, "click here");
        editor.commands.selectAll();
        paste(editor, { text: "https://example.com" });
        expect(getMessageHtml(editor)).toMatch(/<a [^>]*href="https:\/\/example\.com"[^>]*>click here<\/a>/);
    });

    it("keeps links from pasted HTML", () => {
        const editor = makeEditor();
        paste(editor, { html: '<p>go <a href="https://example.com">there</a></p>', text: "go there" });
        const html = getMessageHtml(editor);
        expect(html).toContain('href="https://example.com"');
        expect(html).not.toContain("<p");
    });

    it("does not navigate the webview when a link is clicked", () => {
        const editor = makeEditor();
        type(editor, "https://example.com ");
        const anchor = editor.view.dom.querySelector("a")!;
        const open = vi.spyOn(window, "open").mockImplementation(() => null);
        // jsdom has no layout, so ProseMirror can't map real clicks to a
        // position; invoke the plugins' handleClick directly instead.
        const event = new MouseEvent("click", { bubbles: true, button: 0 });
        Object.defineProperty(event, "target", { value: anchor });
        editor.view.someProp("handleClick", (f) => f(editor.view, 2, event));
        expect(open).not.toHaveBeenCalled();
    });
});

describe("placeholder", () => {
    const placeholder = (editor: Editor) =>
        editor.view.dom.querySelector("[data-placeholder]");

    it("is shown on the empty editor", () => {
        const el = placeholder(makeEditor());
        expect(el).not.toBeNull();
        expect(el!.getAttribute("data-placeholder")).toBe("Type a message...");
        expect(el!.classList.contains("is-editor-empty")).toBe(true);
    });

    it("disappears once there is text", () => {
        const editor = makeEditor();
        type(editor, "x");
        expect(placeholder(editor)).toBeNull();
    });

    it("returns after the editor is cleared", () => {
        const editor = makeEditor();
        type(editor, "x");
        editor.commands.clearContent();
        expect(placeholder(editor)).not.toBeNull();
    });
});

describe("hostile HTML", () => {
    const ALLOWED_TAGS = new Set(["A", "STRONG", "EM", "U", "CODE", "BR"]);
    const ALLOWED_ATTRIBUTES: Record<string, string[]> = {
        A: ["href", "target", "rel"],
    };

    // Whatever goes in, only our small whitelist may come out.
    function expectOnlySafeHtml(html: string) {
        const container = document.createElement("div");
        container.innerHTML = html;
        for (const el of container.querySelectorAll("*")) {
            expect(ALLOWED_TAGS, `<${el.tagName.toLowerCase()}> in ${html}`).toContain(el.tagName);
            for (const attr of el.getAttributeNames()) {
                expect(ALLOWED_ATTRIBUTES[el.tagName] ?? [], `${attr} on <${el.tagName.toLowerCase()}> in ${html}`).toContain(attr);
            }
            if (el.tagName === "A") {
                expect(el.getAttribute("href") ?? "").toMatch(/^(https?:|mailto:)/i);
            }
        }
    }

    const hostile = [
        "<script>alert(1)</script>hi",
        '<img src="https://evil.example/pixel.png">',
        '<img src="data:text/html,<script>alert(1)</script>">',
        '<img src=x onerror="alert(1)">',
        '<svg onload="alert(1)"><circle/></svg>',
        '<iframe src="https://evil.example"></iframe>',
        '<a href="javascript:alert(1)">click</a>',
        '<a href="JaVaScRiPt:alert(1)">click</a>',
        '<a href="data:text/html,<script>alert(1)</script>">click</a>',
        '<a href="https://example.com" onclick="alert(1)" style="color:red">ok</a>',
        '<a href="https://example.com&quot; onmouseover=&quot;alert(1)">quote</a>',
        '<p style="position:fixed" onclick="alert(1)">styled</p>',
        '<strong onmouseover="alert(1)" style="x">bold</strong>',
        '<form action="https://evil.example"><input name=pw></form>',
        '<object data="x"></object><embed src="x">',
        '<style>body{display:none}</style>text',
        '<meta http-equiv="refresh" content="0;url=https://evil.example">',
        '<math><mi xlink:href="javascript:alert(1)">x</mi></math>',
    ];

    it.each(hostile)("setContent(%s) yields only whitelisted HTML", (html) => {
        const editor = makeEditor();
        editor.commands.setContent(html);
        expectOnlySafeHtml(getMessageHtml(editor));
    });

    it.each(hostile)("pasting %s yields only whitelisted HTML", (html) => {
        const editor = makeEditor();
        paste(editor, { html, text: "plain" });
        expectOnlySafeHtml(getMessageHtml(editor));
    });

    it("keeps legitimate links among hostile markup", () => {
        const editor = makeEditor();
        editor.commands.setContent('<script>x</script><a href="https://example.com" onclick="x">ok</a>');
        const html = getMessageHtml(editor);
        expect(html).toContain('href="https://example.com"');
        expect(html).not.toContain("onclick");
    });

    it("escapes markup that is typed as text", () => {
        const editor = makeEditor();
        type(editor, '<script>alert("x")</script> & <b>');
        const html = getMessageHtml(editor);
        expect(html).toBe("&lt;script&gt;alert(\"x\")&lt;/script&gt; &amp; &lt;b&gt;");
        expectOnlySafeHtml(html);
    });

    it("escapes a quote-laden URL typed as text", () => {
        const editor = makeEditor();
        type(editor, 'https://example.com/"onmouseover="alert(1) ');
        expectOnlySafeHtml(getMessageHtml(editor));
    });
});
