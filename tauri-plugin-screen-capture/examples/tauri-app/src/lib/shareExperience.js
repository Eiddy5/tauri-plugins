export function shareExperienceMode({ pickerOpen, session }) {
  if (session) return "sharing"
  return pickerOpen ? "picker" : "landing"
}

export function returnToPickerAfterSharing(state) {
  state.session = null
  state.stats = null
  state.videoReady = false
  state.pickerOpen = true
}
