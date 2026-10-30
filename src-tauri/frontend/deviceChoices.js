// Which audio devices the Settings tab lists, and which of them are ticked.
//
// Kept apart from the DOM so it can be tested: the backend sends Discord's devices and the user's
// choices separately, and a choice can be a device Discord no longer lists (unplugged).
//
// No choices stored means every device Discord offers, which is the default: all of them start
// ticked, and a device plugged in later joins without a visit to Settings.

/// Rows to show: Discord's devices in its order, then the chosen ones it does not list.
export function deviceRows(available, chosen) {
  if (chosen.length === 0) {
    return available.map(device => ({ ...device, checked: true, unplugged: false }));
  }
  const chosenIds = new Set(chosen.map(device => device.id));
  const availableIds = new Set(available.map(device => device.id));
  return [
    ...available.map(device => ({ ...device, checked: chosenIds.has(device.id), unplugged: false })),
    ...chosen
      .filter(device => !availableIds.has(device.id))
      .map(device => ({ ...device, checked: true, unplugged: true })),
  ];
}

/// Whether `row` is the only one ticked. That one cannot be unticked: the shortcut always has
/// somewhere to go.
export function isLastChecked(rows, row) {
  return row.checked && rows.filter(other => other.checked).length === 1;
}

/// What to store: the ticked rows, in the order shown, which is the order the shortcut goes in.
/// Every row ticked is stored as no choice, the default, so later devices are included too.
export function chosenDevices(rows) {
  if (rows.every(row => row.checked && !row.unplugged)) {
    return [];
  }
  return rows.filter(row => row.checked).map(({ id, name }) => ({ id, name }));
}
