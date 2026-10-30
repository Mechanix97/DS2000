// Which audio devices the Settings tab lists, and which of them are ticked.
//
// Kept apart from the DOM so it can be tested: the backend sends Discord's devices and the user's
// choices separately, and a choice can be a device Discord no longer lists (unplugged).

/// Rows to show: Discord's devices in its order, then the chosen ones it does not list.
export function deviceRows(available, chosen) {
  const chosenIds = new Set(chosen.map(device => device.id));
  const availableIds = new Set(available.map(device => device.id));
  return [
    ...available.map(device => ({ ...device, checked: chosenIds.has(device.id), unplugged: false })),
    ...chosen
      .filter(device => !availableIds.has(device.id))
      .map(device => ({ ...device, checked: true, unplugged: true })),
  ];
}

/// What to store: the ticked rows, in the order shown, which is the order the shortcut goes in.
export function chosenDevices(rows) {
  return rows.filter(row => row.checked).map(({ id, name }) => ({ id, name }));
}
