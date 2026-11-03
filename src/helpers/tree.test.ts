import { describe, expect, it, vi } from "vitest";
import type { Channel } from "../invoke/mumbleState";
import { assembleChannelTree, assembleTree } from "./tree";

function channel(channel_id: number, parent: number | null, name = `c${channel_id}`): Channel {
    return { channel_id, parent, name, description: null, is_enter_restricted: false, can_enter: true };
}

describe("assembleTree", () => {
    it("returns a leaf when there are no children", () => {
        expect(assembleTree(1, () => [])).toEqual({ value: 1, children: [] });
    });

    it("recurses through nested children", () => {
        const kids: Record<number, number[]> = { 1: [2, 3], 2: [4] };
        const tree = assembleTree(1, n => kids[n] ?? []);
        expect(tree).toEqual({
            value: 1,
            children: [
                { value: 2, children: [{ value: 4, children: [] }] },
                { value: 3, children: [] },
            ],
        });
    });
});

describe("assembleChannelTree", () => {
    it("throws on an empty array", () => {
        expect(() => assembleChannelTree([])).toThrow("No channels available");
    });

    it("throws when no channel is a root", () => {
        expect(() => assembleChannelTree([channel(1, 2), channel(2, 1)])).toThrow("Root channel not found");
    });

    it("returns a lone root with no children", () => {
        const root = channel(0, null);
        expect(assembleChannelTree([root])).toEqual({ value: root, children: [] });
    });

    it("nests channels under their parents", () => {
        const root = channel(0, null);
        const a = channel(1, 0);
        const b = channel(2, 0);
        const aa = channel(3, 1);
        expect(assembleChannelTree([root, a, b, aa])).toEqual({
            value: root,
            children: [
                { value: a, children: [{ value: aa, children: [] }] },
                { value: b, children: [] },
            ],
        });
    });

    it("throws when there are multiple roots", () => {
        expect(() => assembleChannelTree([channel(0, null), channel(1, null)])).toThrow("Multiple root channels found");
    });

    it("drops orphans whose parent does not exist", () => {
        const root = channel(0, null);
        const tree = assembleChannelTree([root, channel(5, 99)]);
        expect(tree).toEqual({ value: root, children: [] });
    });

    it("ignores a channel that is its own parent", () => {
        const root = channel(0, null);
        const tree = assembleChannelTree([root, channel(1, 1)]);
        expect(tree).toEqual({ value: root, children: [] });
    });

    it("ignores cycles that are not connected to the root", () => {
        const root = channel(0, null);
        const tree = assembleChannelTree([root, channel(1, 2), channel(2, 1)]);
        expect(tree).toEqual({ value: root, children: [] });
    });

    it("keeps siblings in input order", () => {
        const root = channel(0, null);
        const c = channel(3, 0);
        const a = channel(1, 0);
        const b = channel(2, 0);
        const tree = assembleChannelTree([root, c, a, b]);
        expect(tree.children.map(n => n.value)).toEqual([c, a, b]);
    });

    it("calls getChildren once per node in assembleTree", () => {
        const kids: Record<number, number[]> = { 1: [2, 3], 2: [4] };
        const getChildren = vi.fn((n: number) => kids[n] ?? []);
        assembleTree(1, getChildren);
        expect(getChildren).toHaveBeenCalledTimes(4);
    });

    it("handles a deep chain without overflowing the stack", () => {
        const channels = [channel(0, null)];
        for (let i = 1; i < 500; i++) channels.push(channel(i, i - 1));
        let node = assembleChannelTree(channels);
        let depth = 0;
        while (node.children.length) {
            node = node.children[0];
            depth++;
        }
        expect(depth).toBe(499);
    });

    it("finds the root regardless of array order", () => {
        const root = channel(0, null);
        const a = channel(1, 0);
        const tree = assembleChannelTree([a, root]);
        expect(tree.value).toBe(root);
        expect(tree.children.map(c => c.value)).toEqual([a]);
    });
});
