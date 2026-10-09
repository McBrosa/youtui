const YoutuiPlayer = {
  mounted() {
    this.video = this.el.querySelector("video")
    this.placeholder = this.el.querySelector("#player-placeholder")
    this.manualPlay = this.el.querySelector("#player-manual-play")
    this.playbackId = null

    this.onEnded = () => {
      if (this.playbackId !== null && this.video.ended) {
        this.pushEvent("video_ended", { playback_id: this.playbackId })
      }
    }
    this.onError = () => {
      if (this.playbackId !== null && this.video.currentSrc && this.video.error) {
        this.pushEvent("video_error", { playback_id: this.playbackId })
      }
    }
    this.onPlay = () => {
      this.placeholder.hidden = true
      this.manualPlay.hidden = true
    }
    this.onPause = () => {
      if (this.video.ended) this.placeholder.hidden = false
    }
    this.onManualPlay = () => this.video.play().catch(() => {})

    this.video.addEventListener("ended", this.onEnded)
    this.video.addEventListener("error", this.onError)
    this.video.addEventListener("play", this.onPlay)
    this.video.addEventListener("pause", this.onPause)
    this.manualPlay.addEventListener("click", this.onManualPlay)

    this.handleEvent("load_video", ({ url, playback_id }) => {
      this.playbackId = playback_id
      this.video.src = url
      this.video.load()
      this.placeholder.hidden = true
      this.video.play().then(() => {
        if (this.playbackId === playback_id) this.manualPlay.hidden = true
      }).catch(() => {
        if (this.playbackId === playback_id) this.manualPlay.hidden = false
      })
    })

    this.handleEvent("stop_video", () => {
      this.playbackId = null
      this.video.pause()
      this.video.removeAttribute("src")
      this.video.load()
      this.manualPlay.hidden = true
      this.placeholder.hidden = false
    })
  },

  destroyed() {
    this.video.removeEventListener("ended", this.onEnded)
    this.video.removeEventListener("error", this.onError)
    this.video.removeEventListener("play", this.onPlay)
    this.video.removeEventListener("pause", this.onPause)
    this.manualPlay.removeEventListener("click", this.onManualPlay)
  }
}

export default YoutuiPlayer
