<script lang="ts">
  import { browser } from '$app/environment';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';
  export let text = '';
  $: html = browser ? DOMPurify.sanitize(marked.parse(text, { async: false }) as string, {
    ALLOWED_TAGS: ['p', 'br', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'strong', 'em', 'del', 'blockquote', 'ul', 'ol', 'li', 'pre', 'code', 'hr', 'a', 'table', 'thead', 'tbody', 'tr', 'th', 'td'],
    ALLOWED_ATTR: ['href', 'title', 'start'], ALLOW_DATA_ATTR: false
  }) : '';
</script>
<div class="markdown">{@html html}</div>
