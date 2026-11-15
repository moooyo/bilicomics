#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static int device_fd=-1,jpeg,parsed,subscribed,source_requested,capture_requested,source_queued,capture_queued,streamed;
static unsigned int stop_mask,stop_calls;
static struct v4l2_format source_format,capture_format;
static struct v4l2_plane source_plane,capture_planes[3];
static void require(int yes,const char *why){if(!yes){fprintf(stderr,"mock-contract-violation %s\n",why);_exit(95);}}
int open(const char *path,int flags,...){
 int (*real_open)(const char*,int,...)=dlsym(RTLD_NEXT,"open"); mode_t mode=0;
 if(flags&O_CREAT){va_list ap;va_start(ap,flags);mode=va_arg(ap,int);va_end(ap);}
 if(!strcmp(path,"/dev/video1")||!strcmp(path,"/dev/video6")){jpeg=!strcmp(path,"/dev/video1");device_fd=real_open("/dev/null",O_RDWR|O_CLOEXEC);return device_fd;}
 return real_open(path,flags,mode);
}
int ioctl(int fd,unsigned long request,...){
 va_list ap;va_start(ap,request);void *arg=va_arg(ap,void*);va_end(ap);
 int (*real_ioctl)(int,unsigned long,...)=dlsym(RTLD_NEXT,"ioctl");if(fd!=device_fd)return real_ioctl(fd,request,arg);
 if(request==VIDIOC_QUERYCAP){struct v4l2_capability *v=arg;memset(v,0,sizeof(*v));strcpy((char*)v->card,jpeg?"mtk-jdec decoder":"mtk-png decoder");v->capabilities=V4L2_CAP_DEVICE_CAPS;v->device_caps=V4L2_CAP_STREAMING|V4L2_CAP_VIDEO_M2M_MPLANE;return 0;}
 if(request==VIDIOC_SUBSCRIBE_EVENT){require(jpeg&&!parsed,"subscribe before source header");subscribed=1;return 0;}
 if(request==VIDIOC_S_FMT){struct v4l2_format *v=arg;if(V4L2_TYPE_IS_OUTPUT(v->type)){require(!source_requested,"output format before buffers");source_format=*v;}else{require(parsed&&!capture_requested,"capture format only after header and before buffers");capture_format=*v;}return 0;}
 if(request==VIDIOC_G_FMT){struct v4l2_format *v=arg;if(V4L2_TYPE_IS_OUTPUT(v->type))*v=source_format;else{require(parsed,"capture readback requires parsed header");*v=capture_format;const char *inject=getenv("CODEC_MOCK_FAILURE");if(inject&&!strcmp(inject,"changed_planes")&&capture_requested)v->fmt.pix_mp.num_planes=3;}return 0;}
 if(request==VIDIOC_REQBUFS){struct v4l2_requestbuffers *v=arg;require(v->memory==V4L2_MEMORY_USERPTR,"codec USERPTR only");if(V4L2_TYPE_IS_OUTPUT(v->type))source_requested=1;else{require(parsed&&source_queued,"capture buffers allocated after source parsing");capture_requested=1;}v->count=1;return 0;}
 if(request==VIDIOC_QUERYBUF){struct v4l2_buffer *v=arg;unsigned int n=V4L2_TYPE_IS_OUTPUT(v->type)?1:capture_format.fmt.pix_mp.num_planes;require(v->length>=n,"query plane array capacity");v->length=n;for(unsigned int i=0;i<n;++i)v->m.planes[i].length=V4L2_TYPE_IS_OUTPUT(v->type)?16:capture_format.fmt.pix_mp.plane_fmt[i].sizeimage;return 0;}
 if(request==VIDIOC_QBUF){struct v4l2_buffer *v=arg;if(V4L2_TYPE_IS_OUTPUT(v->type)){require(source_requested&&v->length==1&&!capture_requested,"queue first source before capture allocation");source_plane=v->m.planes[0];if(jpeg){require(source_plane.bytesused>=64&&source_plane.bytesused%64==0,"advertise aligned JPEG tail");unsigned char *bytes=(void*)source_plane.m.userptr;for(unsigned int j=source_plane.bytesused-64;j<source_plane.bytesused;++j)require(bytes[j]==0,"JPEG tail must stay zero");}source_queued=1;}else{require(parsed&&capture_requested&&v->length==capture_format.fmt.pix_mp.num_planes,"queue current capture plane count");for(unsigned int i=0;i<v->length;++i){require(v->m.planes[i].m.userptr&&v->m.planes[i].length>=capture_format.fmt.pix_mp.plane_fmt[i].sizeimage,"back every plane");capture_planes[i]=v->m.planes[i];}capture_queued=1;}return 0;}
 if(request==VIDIOC_STREAMON){enum v4l2_buf_type type=*(enum v4l2_buf_type*)arg;if(V4L2_TYPE_IS_OUTPUT(type)){require(source_queued&&(!jpeg||subscribed),"header stream setup");parsed=1;memset(&capture_format,0,sizeof(capture_format));capture_format.type=V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;struct v4l2_pix_format_mplane *p=&capture_format.fmt.pix_mp;p->width=source_format.fmt.pix_mp.width;p->height=source_format.fmt.pix_mp.height;p->num_planes=jpeg?2:1;p->pixelformat=jpeg?V4L2_PIX_FMT_NV12:V4L2_PIX_FMT_ARGB32;for(unsigned int i=0;i<p->num_planes;++i){p->plane_fmt[i].bytesperline=jpeg?(i?(p->width+1)/2:p->width):p->width*4;p->plane_fmt[i].bytesperline=(p->plane_fmt[i].bytesperline+15)&~15U;p->plane_fmt[i].sizeimage=(p->plane_fmt[i].bytesperline*p->height+63)&~63U;}}else{require(parsed&&capture_queued,"capture streams only with header-matched allocation");streamed=1;}return 0;}
 if(request==VIDIOC_DQEVENT){require(jpeg&&parsed,"JPEG source event after parse");struct v4l2_event *v=arg;memset(v,0,sizeof(*v));v->type=V4L2_EVENT_SOURCE_CHANGE;v->u.src_change.changes=V4L2_EVENT_SRC_CH_RESOLUTION;return 0;}
 if(request==VIDIOC_DQBUF){require(streamed,"decode before buffer completion");struct v4l2_buffer *v=arg;v->index=0;v->flags=V4L2_BUF_FLAG_DONE;if(V4L2_TYPE_IS_OUTPUT(v->type)){v->length=1;v->m.planes[0]=source_plane;}else{v->length=capture_format.fmt.pix_mp.num_planes;for(unsigned int i=0;i<v->length;++i){unsigned int size=capture_format.fmt.pix_mp.plane_fmt[i].sizeimage;memset((void*)capture_planes[i].m.userptr,128,size);v->m.planes[i]=capture_planes[i];v->m.planes[i].bytesused=size;const char *inject=getenv("CODEC_MOCK_FAILURE");if(inject&&!strcmp(inject,"oversized_payload"))v->m.planes[i].bytesused=capture_planes[i].length+1;}}return 0;}
 if(request==VIDIOC_STREAMOFF){const char *failure=getenv("CODEC_MOCK_FAILURE");unsigned int call=stop_calls++;if(failure&&!strcmp(failure,"stop_forever")){errno=EIO;return -1;}if(failure&&call==0&&(!strcmp(failure,"stop_eintr")||!strcmp(failure,"stop_io_once"))){errno=!strcmp(failure,"stop_eintr")?EINTR:EIO;return -1;}stop_mask|=V4L2_TYPE_IS_OUTPUT(*(enum v4l2_buf_type*)arg)?1:2;return 0;}
 errno=ENOTTY;return -1;
}

int close(int fd){int (*real_close)(int)=dlsym(RTLD_NEXT,"close");if(fd==device_fd){require(!streamed||stop_mask==3,"both streams stopped before device close");fprintf(stderr,"MOCK_DEVICE_CLOSED\n");device_fd=-1;}return real_close(fd);}
