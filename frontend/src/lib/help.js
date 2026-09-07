/**
 * In-app documentation.
 *
 * Content as data rather than markup so it can be searched, and so the modal
 * stays a renderer. Each topic is a heading plus prose, optional numbered
 * steps, and an optional aside — enough shape for a help page, not so much
 * that writing a paragraph means writing HTML.
 *
 * Keep it about what the user can see and do. Anything about how it is built
 * belongs in the README.
 */

export const HELP = [
  {
    id: 'basics',
    title: 'Getting started',
    icon: 'home',
    topics: [
      {
        title: 'One account, six apps',
        body: [
          'Mail, Calendar, Contacts, Notes and Files are all views of the same account on your mail server. There is nothing separate to sign into and nothing to sync — a file you upload is on the server the moment the upload finishes.',
          'The bar along the top switches between them. On a narrow screen the app names collapse into a menu behind the current app’s name.',
        ],
      },
      {
        title: 'The dashboard',
        body: [
          'The house icon opens an overview: unread mail, storage used, today’s events, and the files and notes you touched most recently. Everything on it is a link — clicking a message opens it in Mail, clicking a file reveals it in Files.',
        ],
      },
      {
        title: 'Finding things quickly',
        body: [
          'Press Ctrl+K (⌘K on a Mac) anywhere to open the command palette. It searches your apps, folders, contacts, calendars and files at once, and also runs commands like composing a message or switching the theme. Start typing and press Enter.',
        ],
        tip: 'The palette matches on fragments, so “dnfld” finds “Dune Field.png”.',
      },
      {
        title: 'Dark mode',
        body: [
          'The sun/moon button in the top bar switches themes. It follows your operating system’s setting until you choose one explicitly, and remembers your choice on that device afterwards.',
        ],
      },
      {
        title: 'Installing it as an app',
        body: [
          'This is a progressive web app: your browser can install it to your dock, taskbar or home screen, where it opens in its own window without browser chrome. Look for “Install” or “Add to Home Screen” in your browser’s menu.',
        ],
      },
    ],
  },

  {
    id: 'mail',
    title: 'Mail',
    icon: 'mail',
    topics: [
      {
        title: 'Reading',
        body: [
          'Three panes: folders, the message list, and the message itself. Below about 1024 pixels wide they become one pane at a time, and you drill down and come back.',
          'j and k move through the list, u marks read or unread, # deletes. Press / to search. Escape closes the message or clears a selection.',
        ],
      },
      {
        title: 'Writing',
        body: [
          'c composes, r replies, a replies to everyone, f forwards. Ctrl+Enter (⌘Enter) sends.',
          'The toolbar has bold, italic, underline, strikethrough, lists, links and inline images. Closing a message you have started writing asks before discarding it.',
        ],
      },
      {
        title: 'Attaching files',
        body: [
          'Two buttons in the compose toolbar, because attachments come from two places. The paperclip takes files from this device — you can also drag them straight onto the compose window. The folder button opens your Files drive and attaches something already stored there.',
          'A file from your drive is attached by reference. Nothing is downloaded and re-uploaded, so attaching a large file is instant however big it is.',
          'Attachments are capped at 25 MB in total for the message, which is roughly where receiving servers start refusing mail.',
        ],
        tip: 'Sending is held back while an upload is still running, so a message can never go out missing its attachments.',
      },
      {
        title: 'Filters and rules',
        body: [
          'Your avatar menu → Filters / Rules builds rules from conditions (From, To, Cc, Subject) and actions (move to a folder, mark read, discard). They run on the server, so they apply to mail arriving through any client, not just this one.',
        ],
      },
      {
        title: 'Why images in mail do not load',
        body: [
          'Remote images are blocked until you ask for them. A picture loaded from a sender’s server tells them you opened the message, when you opened it and roughly where you were — a read receipt you never agreed to.',
          'Use “Show images” on a message you trust, or trust the sender to load them automatically from then on.',
        ],
      },
      {
        title: 'Organising',
        body: [
          'Drag a message onto a folder to move it, or right-click for reply, forward, read/unread, move and delete. Select several with the checkboxes and the same menu acts on all of them.',
          'The + beside Folders creates one.',
        ],
      },
    ],
  },

  {
    id: 'files',
    title: 'Files',
    icon: 'folder',
    topics: [
      {
        title: 'Getting files in',
        body: [
          'Drag files anywhere onto the listing, or use Upload. Dragging a whole folder works too — the folder structure is recreated as it uploads, and each file gets its own progress bar.',
          'Uploading two files with the same name into one folder does not overwrite: the second becomes “name (2)”.',
        ],
      },
      {
        title: 'Looking at things',
        body: [
          'Images show their own contents in place of an icon, in both the grid and the list, so a folder of photographs is readable at a glance.',
          'Clicking a file previews it. Images, PDFs, text, audio and video open in the app; anything else downloads, because previewing it safely is not something the server can promise.',
        ],
        tip: 'Very large images keep a plain icon rather than a thumbnail — there is no thumbnail service, so a preview means downloading the whole picture.',
      },
      {
        title: 'Editing text without leaving',
        body: [
          'Open a text or Markdown file and press Edit. Save writes it back as a new version of the same file, so anything pointing at it keeps working.',
        ],
      },
      {
        title: 'The right-click menu',
        body: [
          'Right-click any file or folder for preview, open in a new tab, download, share by email, rename, select and delete. Right-clicking inside a multi-selection acts on the whole selection.',
          'Right-clicking empty space gives you Upload and New folder for the folder you are in.',
        ],
      },
      {
        title: 'Sharing a file by email',
        body: [
          '“Share via email” on any file opens a new message with that file already attached, without leaving Files. Because the file is already on the server it is attached by reference — no upload, no wait.',
        ],
      },
      {
        title: 'Moving, renaming and searching',
        body: [
          'Drag items onto a folder in the listing or the sidebar tree to move them. The pencil renames. Search covers the whole drive rather than the open folder, so it finds things you have forgotten the location of.',
          'Deleting a folder deletes what is inside it.',
        ],
      },
    ],
  },

  {
    id: 'notes',
    title: 'Notes',
    icon: 'note',
    topics: [
      {
        title: 'Notes are just files',
        body: [
          'Every note is a Markdown file in a Notes folder in your drive. Point any WebDAV client or sync tool at the same account and you have the same notes — and an existing folder of Markdown works here untouched.',
        ],
      },
      {
        title: 'Writing',
        body: [
          'Type in Markdown and use the toolbar for headings, bold, italic, lists, quotes, links and code. There is a preview, and a split view for both at once. Changes save on their own.',
          'A note’s title comes from its first heading, or its front matter if it has any.',
        ],
      },
      {
        title: 'Images in notes',
        body: [
          'A relative image link is resolved against the note’s own folder, so attachment folders written by other Markdown editors display correctly rather than as broken links.',
        ],
      },
    ],
  },

  {
    id: 'calendar',
    title: 'Calendar',
    icon: 'calendar',
    topics: [
      {
        title: 'Events',
        body: [
          'A month at a time. Click a day to add an event, or an event to edit or delete it. An event has a title, time or all-day span, location, description, status and an optional repeat.',
          'The sidebar lists your calendars; use it to show one at a time or all together.',
        ],
      },
    ],
  },

  {
    id: 'contacts',
    title: 'Contacts',
    icon: 'contacts',
    topics: [
      {
        title: 'Your address books',
        body: [
          'Contacts hold names, organisations, job titles, several email addresses and phone numbers, and postal addresses. Search covers every field, not just the name.',
          'A contact’s email address is a link — clicking it starts a message.',
        ],
      },
      {
        title: 'Importing',
        body: [
          'Drop a .vcf file onto Contacts, or use the import button, to bring in an address book exported from another client. You are shown what was found before anything is saved.',
        ],
      },
    ],
  },

  {
    id: 'security',
    title: 'Privacy and security',
    icon: 'shield',
    topics: [
      {
        title: 'Mail cannot run code',
        body: [
          'Messages are displayed in a sandbox that cannot execute scripts or reach the rest of the app, and their content is cleaned before it gets there. A hostile message cannot read your mail or act as you.',
        ],
      },
      {
        title: 'Senders cannot track you by default',
        body: [
          'Remote images, background images and remote styles are stripped until you ask for them, per message or per sender. See “Why images in mail do not load”.',
        ],
      },
      {
        title: 'Links open away from the app',
        body: [
          'Every link in a message opens in a new tab with no access back to this page, and carries no referrer.',
        ],
      },
      {
        title: 'App passwords',
        body: [
          'Your avatar menu → App Passwords creates a separate password for a phone’s mail app or a desktop client, so your main credentials never go into another program. Revoke one at any time without touching the others.',
          'The password is shown once, when you create it. Copy it then.',
        ],
      },
      {
        title: 'Signing out',
        body: [
          'Sign out from your avatar menu. It clears the session on the server, not just in this browser.',
        ],
        tip: 'On a shared computer, sign out rather than only closing the tab.',
      },
    ],
  },
];

/** Everything a topic can be matched on, lowercased once. */
function haystack(topic, sectionTitle) {
  return [
    sectionTitle,
    topic.title,
    ...(topic.body ?? []),
    topic.tip ?? '',
  ].join(' ').toLowerCase();
}

/**
 * Filter the documentation down to topics matching `query`.
 *
 * Every whitespace-separated term must appear somewhere in the topic, so
 * adding a word narrows rather than widens — the behaviour someone expects
 * from a search box when the first attempt returns too much.
 *
 * Sections with no surviving topics are dropped, so the navigation and the
 * content never disagree about what there is to read.
 *
 * @returns {Array} the same shape as HELP, filtered
 */
export function searchHelp(sections, query) {
  const terms = String(query ?? '').toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return sections ?? [];

  const out = [];
  for (const section of sections ?? []) {
    const topics = (section.topics ?? []).filter((topic) => {
      const text = haystack(topic, section.title);
      return terms.every((term) => text.includes(term));
    });
    if (topics.length) out.push({ ...section, topics });
  }
  return out;
}

/** Total topics, for the "no results" line and for tests. */
export function countTopics(sections) {
  return (sections ?? []).reduce((n, s) => n + (s.topics?.length ?? 0), 0);
}
