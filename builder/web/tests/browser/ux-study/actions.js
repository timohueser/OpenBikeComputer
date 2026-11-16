// The actions that a participant may use in the browser: their schema, their description for the prompt,
// the check of their fields, and when two of them are the same.

export const VIEWPORT = { width: 1280, height: 800 };
const SAME_POINT_PX = 20;

const point = { anyOf: [{ type: 'integer' }, { type: 'null' }] };
const nullableText = { anyOf: [{ type: 'string' }, { type: 'null' }] };

/** The `action` part of the step record schema. */
const schema = {
  type: 'object',
  properties: {
    type: { type: 'string', enum: ['click', 'drag', 'scroll', 'hover', 'type', 'key', 'give_up'] },
    target: { type: 'string' },
    x: point, y: point, x2: point, y2: point,
    text: nullableText,
  },
  required: ['type', 'target', 'x', 'y', 'x2', 'y2', 'text'],
  additionalProperties: false,
};

const text = `The screenshot is ${VIEWPORT.width} × ${VIEWPORT.height} pixels. A point is x, y in pixels from the top left.
- click: click at x, y.
- drag: press at x, y, move to x2, y2, release.
- scroll: turn the mouse wheel at x, y; text is "up" or "down".
- hover: move the mouse to x, y.
- type: click at x, y, then type text.
- key: press one key, such as "Enter", "Escape" or "Tab"; text is the key name.
- give_up: stop trying.
Name the target in words. Use null for each field that the action does not use.`;

const onScreen = (x, y) => Number.isInteger(x) && Number.isInteger(y)
  && x >= 0 && x < VIEWPORT.width && y >= 0 && y < VIEWPORT.height;
const filled = value => typeof value === 'string' && value !== '';

/** True when `action` has the fields that its type needs, so the browser can perform it. */
function valid({ type, x, y, x2, y2, text }) {
  switch (type) {
    case 'click': case 'hover': return onScreen(x, y);
    case 'drag': return onScreen(x, y) && onScreen(x2, y2);
    case 'scroll': return onScreen(x, y) && (text === 'up' || text === 'down');
    case 'type': return filled(text) && ((x === null && y === null) || onScreen(x, y));
    case 'key': return filled(text);
    case 'give_up': return true;
    default: return false;
  }
}

/** True when two actions are the same: the same type and text, and each point within 20 pixels. */
function same(a, b) {
  const near = (x1, y1, x2, y2) => (x1 === null || x2 === null)
    ? x1 === x2 && y1 === y2
    : Math.hypot(x1 - x2, y1 - y2) <= SAME_POINT_PX;
  return a.type === b.type && (a.text ?? null) === (b.text ?? null)
    && near(a.x, a.y, b.x, b.y) && near(a.x2, a.y2, b.x2, b.y2);
}

export const actions = { schema, text, valid, same };
