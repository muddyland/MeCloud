import { writable } from 'svelte/store';

export const addressBooks       = writable([]);
export const contacts           = writable([]);
export const selectedAddressBook = writable(null); // null = all
export const selectedContact    = writable(null);
export const contactsLoading    = writable(false);
export const contactSearch      = writable('');
export const contactModalOpen   = writable(false);
export const editingContact     = writable(null); // null = new contact
