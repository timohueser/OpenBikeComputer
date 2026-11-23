/** happy-dom has no Web Animations. Here an animation ends at once, so a transition ends and its element leaves. */
Element.prototype.animate ??= function () {
    const animation = { onfinish: null as (() => void) | null, cancel() {}, currentTime: 0 };
    queueMicrotask(() => animation.onfinish?.());
    return animation as unknown as Animation;
};
