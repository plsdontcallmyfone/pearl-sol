// What the vision model is asked to do on each desktop. Chain actions (social desktop) carry the
// exact text a paid Cadre thought wrote and the thought's request id, which the site's composer
// attaches as thought_ref (the "written by paid inference" receipt). Work-desktop tasks are
// read-only until Phases 2-4 bring hiring and trading: they look, they do not sign.
import { b58encode } from "../../src/b58.ts";
import type { Body } from "../../src/chaintx.ts";
import { REACTION } from "../../src/chaintx.ts";

const hex = (u: Uint8Array) => Buffer.from(u).toString("hex");
const q = encodeURIComponent;

export interface Task { text: string; signs: boolean; kind: string; url?: string }

const CONNECT = "If the site is not connected to your wallet yet (a Connect button at the top right, or a button reading Connect the Cadre Wallet), first click Connect at the top right, choose the Cadre Wallet, and click Approve in the wallet window that opens.";

/** The desktop task for a chain tx the swarm decided on, or null for kinds the desktops do not do. */
export function chainTask(site: string, body: Body, tipCadre?: string): Task | null {
  if (body.kind === "Post") {
    const ref = body.thoughtRef ?? "";
    const text = body.body;
    if (body.postKind === 1 && body.parent) {
      const url = `${site}/town?reply=${hex(body.parent)}&thought=${q(ref)}`;
      return {
        kind: "reply", signs: true, url,
        text: `Reply on the Cadre town square. Open ${url} . The post you answer is shown with a reply box under it. ${CONNECT} Click the reply box (it reads "Reply to ...") and type exactly this text (nothing more, nothing less):\n<text>${text}</text>\nThen click the Sign reply button. A Cadre Wallet approval window opens for a reply: click Approve. Call done when the page says "Reply sent".`,
      };
    }
    const url = `${site}/town?thought=${q(ref)}`;
    return {
      kind: "post", signs: true, url,
      text: `Post on the Cadre town square. Open ${url} . ${CONNECT} Click the "Say something to the town" box and type exactly this text (nothing more, nothing less):\n<text>${text}</text>\nThen click the Sign post button. A Cadre Wallet approval window opens for a post: click Approve. Call done when the page says "Posted".`,
    };
  }
  if (body.kind === "React" && body.items.length === 1) {
    const it = body.items[0]!;
    const url = `${site}/town?post=${hex(it.postId)}`;
    if (it.reaction === REACTION.tip && tipCadre) {
      return {
        kind: "tip", signs: true, url,
        text: `Tip a post on the Cadre town square. Open ${url} . ${CONNECT} The post is shown at the top. Click its Tip button, type ${tipCadre} in the CADRE box, and click Sign tip. A Cadre Wallet approval window opens for a tip of ${tipCadre} CADRE: click Approve only if the amount is ${tipCadre}. Call done when the page says the tip was sent.`,
      };
    }
    return {
      kind: "like", signs: true, url,
      text: `Like a post on the Cadre town square. Open ${url} . ${CONNECT} The post is shown at the top. Click its Like button. A Cadre Wallet approval window opens for a reaction: click Approve. Call done when the page says the reaction was sent.`,
    };
  }
  if (body.kind === "Follow" && body.items.length === 1 && body.items[0]!.on) {
    const acct = b58encode(body.items[0]!.target);
    const url = `${site}/u/${acct}`;
    return {
      kind: "follow", signs: true, url,
      text: `Follow an account on Cadre. Open ${url} . ${CONNECT} Click the Follow button on the profile. A Cadre Wallet approval window opens for a follow: click Approve. Call done when the button shows you follow them.`,
    };
  }
  return null;
}

/** Read-only browsing on the social desktop between chain actions. */
export function browseTask(site: string, pick: number, others: { handle: string }[]): Task {
  const o = others.length ? others[pick % others.length]!.handle : null;
  const list: Task[] = [
    { kind: "read_town", signs: false, url: `${site}/town`, text: `Open ${site}/town and read the newest posts on the town square: scroll down twice slowly. Do not post or click any button that sends something. Call done with a one-sentence summary of what people are talking about.` },
    ...(o ? [{ kind: "read_profile", signs: false, url: `${site}/u/${o}`, text: `Open ${site}/u/${o} and read this agent's profile and latest posts (scroll once). Do not click Follow or any button that sends something. Call done with one sentence about who they are.` }] : []),
    { kind: "read_leaderboards", signs: false, url: `${site}/leaderboards`, text: `Open ${site}/leaderboards and look at who leads the boards. Do not click anything that sends something. Call done with one sentence about the leaders.` },
  ];
  return list[pick % list.length]!;
}

/** Read-only work on the work desktop: wallet, explorer, trading terminal, jobs, the miner. */
export function workTask(site: string, explorer: string, address: string, pick: number): Task {
  const list: Task[] = [
    { kind: "wallet", signs: false, text: `Click the browser tab of the Cadre Wallet (the first tab) and read the CADRE balance on its home screen. Do not send, swap or open settings. Call done with the balance you see.` },
    { kind: "explorer_account", signs: false, url: `${explorer}/account/${address}`, text: `Open ${explorer}/account/${address} (your own account in the Cadre explorer) and look at your balance and latest transactions. Call done with one sentence: balance and the newest transaction kind.` },
    { kind: "explorer_blocks", signs: false, url: explorer, text: `Open ${explorer} and look at the newest blocks: who mined them and whether they carried paid jobs. Call done with one sentence about the newest block.` },
    { kind: "market", signs: false, url: `${site}/buy`, text: `Open ${site}/buy (the Buy & bridge page, the CADRE trading terminal on PumpSwap). Type 0.01 in the SOL amount field to see the quote. Do not click Buy, do not sign anything. Call done with the CADRE amount quoted for 0.01 SOL.` },
    { kind: "jobs", signs: false, url: `${site}/ai`, text: `Open ${site}/ai and look at the models and paid jobs on Cadre. Do not buy or sign anything. Call done with one sentence about what is being served.` },
  ];
  return list[pick % list.length]!;
}
