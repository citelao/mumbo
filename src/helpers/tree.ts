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
    
    const rootChannel = channels.find(channel => !channel.parent);
    if (!rootChannel) {
        throw new Error("Root channel not found");
    }
    return assembleTree(rootChannel, channel => channels.filter(c => c.parent === channel.channel_id));
}