import { describe, it, expect } from 'vitest';
import { HELP, searchHelp, countTopics } from '$lib/help.js';

const sections = [
  {
    id: 'mail', title: 'Mail', icon: 'mail',
    topics: [
      { title: 'Writing', body: ['c composes, r replies.'] },
      { title: 'Attaching files', body: ['The paperclip takes files from this device.'],
        tip: 'Sending is held back while an upload is running.' },
    ],
  },
  {
    id: 'files', title: 'Files', icon: 'folder',
    topics: [{ title: 'Getting files in', body: ['Drag files onto the listing.'] }],
  },
];

describe('searchHelp', () => {
  it('returns everything for an empty query', () => {
    expect(searchHelp(sections, '')).toBe(sections);
    expect(searchHelp(sections, '   ')).toBe(sections);
    expect(searchHelp(sections, null)).toBe(sections);
  });

  it('matches topic titles and body text', () => {
    expect(countTopics(searchHelp(sections, 'paperclip'))).toBe(1);
    expect(countTopics(searchHelp(sections, 'writing'))).toBe(1);
  });

  it('matches the tip, which is real content and not decoration', () => {
    const found = searchHelp(sections, 'upload');
    expect(found[0].topics[0].title).toBe('Attaching files');
  });

  it('matches the section title, so "mail" finds what lives under Mail', () => {
    expect(countTopics(searchHelp(sections, 'mail'))).toBe(2);
  });

  it('is case-insensitive', () => {
    expect(countTopics(searchHelp(sections, 'PAPERCLIP'))).toBe(1);
  });

  it('narrows with each extra term rather than widening', () => {
    // Every term must appear, which is what a search box is expected to do.
    // "Attaching files" by its title, "Getting files in" by its section.
    expect(countTopics(searchHelp(sections, 'files'))).toBe(2);
    expect(countTopics(searchHelp(sections, 'files paperclip'))).toBe(1);
    expect(countTopics(searchHelp(sections, 'files nonsense'))).toBe(0);
  });

  it('drops sections with no surviving topics', () => {
    // Otherwise the navigation offers a section whose body is empty.
    const found = searchHelp(sections, 'paperclip');
    expect(found).toHaveLength(1);
    expect(found[0].id).toBe('mail');
  });

  it('does not mutate the input', () => {
    const before = JSON.stringify(sections);
    searchHelp(sections, 'paperclip');
    expect(JSON.stringify(sections)).toBe(before);
  });

  it('survives absent input', () => {
    expect(searchHelp(undefined, 'x')).toEqual([]);
    expect(searchHelp([], 'x')).toEqual([]);
    expect(countTopics(undefined)).toBe(0);
  });
});

describe('the shipped documentation', () => {
  it('has a unique id and at least one topic per section', () => {
    const ids = HELP.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const section of HELP) {
      expect(section.title, section.id).toBeTruthy();
      expect(section.topics.length, section.id).toBeGreaterThan(0);
    }
  });

  it('gives every topic a title and some prose', () => {
    for (const section of HELP) {
      for (const topic of section.topics) {
        expect(topic.title, section.id).toBeTruthy();
        expect(topic.body?.length, `${section.id}/${topic.title}`).toBeGreaterThan(0);
        for (const para of topic.body) expect(typeof para).toBe('string');
      }
    }
  });

  it('covers every app the navigation offers', () => {
    const ids = HELP.map((s) => s.id);
    for (const app of ['mail', 'files', 'notes', 'calendar', 'contacts']) {
      expect(ids).toContain(app);
    }
  });
});
