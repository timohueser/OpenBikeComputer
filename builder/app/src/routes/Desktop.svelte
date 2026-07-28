<!--
  Where every gate's next step leads (#901). The list is derived from the same
  requirements the gates are, so the page can't promise something the tier
  already has, or quietly forget something it doesn't — and on Chromium it
  leaves USB off, because on Chromium the site does USB.
-->
<script lang="ts">
    import { LINKS } from "../lib/constants";
    import { RELEASE } from "../lib/desktop/release";
    import { formatBytes } from "../lib/format";
    import { DESKTOP_ADDS, GATES } from "../lib/platform/gating";
</script>

<article>
    <h1>The desktop app</h1>
    <p class="lead muted">
        The same builder, running on your own machine — with the parts that need real CPU, a real
        folder, or a USB driver a browser doesn't have.
    </p>

    <section class="card">
        <h2>What it adds</h2>
        <dl>
            {#each DESKTOP_ADDS as need (need)}
                <dt>{GATES[need].title}</dt>
                <dd class="muted">{GATES[need].offer}</dd>
            {/each}
        </dl>
    </section>

    <section class="card">
        <h2>Downloads</h2>
        {#if RELEASE}
            <p class="small faint">Version {RELEASE.version} · {RELEASE.date}</p>
            <ul class="downloads">
                {#each RELEASE.downloads as file (file.filename)}
                    <li>
                        <a class="btn ghost" href={file.url} download={file.filename}>
                            {file.os}{file.arch ? ` · ${file.arch}` : ""}
                        </a>
                        <span class="small muted">{file.filename} · {formatBytes(file.size)}</span>
                        <code class="mono small sum">{file.sha256}</code>
                    </li>
                {/each}
            </ul>
            {#if RELEASE.installNote}
                <h3>First run</h3>
                <p class="small">{RELEASE.installNote}</p>
            {/if}
        {:else}
            <p>
                There are no builds yet. The macOS, Windows and Linux installers will appear on the
                releases page, each with a SHA-256 to check it against.
            </p>
            <p class="after"><a href={LINKS.releases}>Releases →</a></p>
        {/if}
    </section>
</article>

<style>
    article {
        width: min(760px, 100%);
        margin: 0 auto;
        display: flex;
        flex-direction: column;
        gap: 14px;
    }

    h1 {
        font-size: 26px;
    }

    h2 {
        font-size: 17px;
        margin-bottom: 10px;
    }

    h3 {
        font-size: 14.5px;
        margin: 14px 0 4px;
    }

    .lead {
        margin: 4px 0 4px;
        max-width: 62ch;
    }

    dl {
        margin: 0;
        display: grid;
        gap: 10px;
    }

    dt {
        font-weight: 600;
    }

    dd {
        margin: 1px 0 0;
        font-size: 14px;
        max-width: 68ch;
    }

    .downloads {
        list-style: none;
        margin: 10px 0 0;
        padding: 0;
        display: grid;
        gap: 12px;
    }

    .downloads li {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        gap: 4px 10px;
    }

    .sum {
        flex-basis: 100%;
        color: var(--ink-faint);
        word-break: break-all;
    }

    p {
        margin: 0;
    }

    .after {
        margin-top: 10px;
    }
</style>
