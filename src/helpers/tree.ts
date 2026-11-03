import { Channel } from "../invoke/mumbleState";

export type TreeNode<T> = {
    value: T;
    children: TreeNode<T>[];
};

export type GetChildrenFn<T> = (value: T) => T[];
export function assembleTree<T>(rootNode: T, getChildren: GetChildrenFn<T>): TreeNode<T> {
    const children = getChildren(rootNode).map(child => assembleTree(child, getChildren));
    return {
        value: rootNode,
        children: children
    };
}

export function assembleChannelTree(channels: Channel[]): TreeNode<Channel> {
    if (channels.length === 0) {
        throw new Error("No channels available");
    }
    
    // Compare to null explicitly: the root's id is 0, so its children have
    // `parent: 0`, which is falsy.
    const roots = channels.filter(channel => channel.parent === null);
    if (roots.length === 0) {
        throw new Error("Root channel not found");
    }
    if (roots.length > 1) {
        throw new Error("Multiple root channels found");
    }
    const rootChannel = roots[0];
    return assembleTree(rootChannel, channel => channels.filter(c => c.parent === channel.channel_id));
}