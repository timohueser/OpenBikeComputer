# Product

OpenBikeComputer is an open-source GPS computer for bikepacking: a device on the handlebar, a companion app for iOS, and a route planner in the browser.

## Platform

- **The device:** a 240 × 320 reflective display with buttons. A rider reads it in a moment, while riding.
- **The companion app (iOS):** sends a route to the device and brings the recorded ride back.
- **The route planner (web):** plans a route on a map.

## Users

Riders who travel by bike for several days. They plan a route before the ride, and on the ride they keep the phone in the pocket. They use no account and no subscription.

## Product Purpose

Map-based tracking and navigation that is reliable and simple. A route reaches the device every time. The device shows what a rider checks often: the route, the places near it and along it, and the climb ahead.

## Product Principles

The core principles decide when two of the others disagree.

### Core

1. **Obvious at first use, fast at the hundredth.** An interaction does what the rider expects, so a new rider finds their way with no manual. The same interaction lets an experienced rider make an edit quickly.
2. **Simplicity through gradual revelation.** Complexity appears when the rider asks for it, never on the main path. What a rider needs early is never hidden.
3. **Fluid: the rider floats through it.** Screens, components and features link to each other in a way that the rider can see, so that a move between them feels natural. A transition shows where a thing came from. A thing that two screens share keeps its look and its place. The rider always knows where they are and how to go back.
4. **Every element earns its place.** Remove before you add. The interface steps back for the content. Text is a last resort: show, do not explain.
5. **One obvious primary action per screen.** The eye lands on one thing to do. Secondary actions look secondary.

### The others

- **No hidden modes.** The current state is always visible. Prefer a control in the page to a dialog, an alert or a popup.
- **Immediate, visible feedback.** The interface answers each action at once, and shows progress when the action takes longer.
- **Recognition over recall.** Show the options; do not make the rider remember them. Offer the recent and the likely choices.
- **Forgiveness.** A destructive action has an undo. A question "Are you sure?" is rare.
- **Decide for the rider where it is safe.** A good default, or a value from the context, in place of a question.
- **Consistent inside the product and with the platform.** One concept has one name and one place. Follow the convention of the platform unless another way is clearly better.
- **Natural mapping.** A control is near the thing it changes, and looks like it.
- **The rider's model, not the implementation's.** No term of a database, an API or the code in the interface.
- **Direct and interruptible.** The rider moves the thing itself. Motion follows physics, and the rider can interrupt it.
- **Prevent errors first, then explain them.** Constraints and forgiving input. An error message says what happened and what to do, in plain words.
- **Never lose the rider's work or place.** Drafts stay, state comes back, an interruption does no harm.
- **Visual hierarchy matches importance.** The most important thing is seen first. Related things look grouped. A frequent action has a large target that is near.
- **Every action has a visible signifier.** No feature that only a gesture reaches.
- **The first run teaches itself.** Empty states and defaults show what to do. No manual and no tutorial.
- **Fast paths for regulars.** Shortcuts and remembered choices, out of the way of a newcomer.

## Accessibility & Inclusion

Accessible by default: text of a legible size, enough contrast, no state that only a colour shows, and a label on each control for assistive technology.
